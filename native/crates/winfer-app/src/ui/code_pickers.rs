//! The model, effort and mode pickers of the chat pane: popovers that open upward from their
//! buttons on the composer's bottom bar. The model picker has a backend rail, a search field,
//! favourites and a reasoning column; the effort picker is its own component (`EffortPicker`).

use super::code_view::{CodeView, rise};
use super::nebula;
use crate::code::names::{BACKENDS, ModelTier, backend_of, base_spec, fast_spec, is_fast, model_name, model_tier, supports_ultracode};
use crate::code::protocol::*;
use crate::code::state::Sheet;
use crate::theme::{self, Colors};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Levels offered when the model doesn't list its own.
pub const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

const FAVORITES: &str = "★";
const SHOWN: usize = 8;

fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

pub fn effort_label(level: &str) -> String {
    match level {
        "default" => "Default".into(),
        "xhigh" => "Extra high".into(),
        l => l.chars().next().map_or(String::new(), |c| c.to_uppercase().chain(l.chars().skip(1)).collect()),
    }
}

pub fn effort_hint(level: &str) -> &'static str {
    match level {
        "default" => "The model's own default",
        "minimal" => "Barely any reasoning, fastest",
        "low" => "Quick answers, light reasoning",
        "medium" => "Balanced",
        "high" => "Thorough reasoning",
        "xhigh" => "Very thorough, slower",
        "max" => "As much reasoning as the model can do",
        "ultra" => "Beyond max, for the hardest problems",
        _ => "",
    }
}

/// A filled star (favorites, defaults), in one colour.
pub fn star_icon(tint: Hsla, size: f32) -> AnyElement {
    svg().path("solid/star.svg").size(px(size)).flex_shrink_0().text_color(tint).into_any_element()
}

/// A level's solid glyph in one colour: rising bars for the steps, a flame, a bolt and a sparkle above.
pub fn effort_glyph(tint: Hsla, level: &str, size: f32) -> AnyElement {
    let solid = |file: &str| svg().path(SharedString::from(format!("solid/{file}.svg"))).size(px(size)).flex_shrink_0().text_color(tint).into_any_element();
    let filled = match level {
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        _ => 0,
    };
    match level {
        "max" => solid("flame"),
        "ultra" => solid("bolt"),
        "default" => solid("sparkle"),
        _ => div().w(px(size)).h(px(size)).flex().items_end().justify_center().gap(px(size * 0.1)).children((1..=5).map(|b| {
            div().w(px(size * 0.14)).h(px(size * (0.22 + 0.15 * b as f32))).rounded_sm().bg(if b <= filled { tint } else { alpha(tint, 0.25) })
        })).into_any_element(),
    }
}

/// Matches a model spec in `models`, also falling back to stripping any legacy trailing effort
/// suffix (`agy/gemini-4-neon-xhigh` → `agy/gemini-4-neon`).
pub(super) fn match_model<'a>(models: &'a [ModelInfo], spec: &str) -> Option<&'a ModelInfo> {
    let target = base_spec(spec);
    models.iter().find(|i| base_spec(&i.spec) == target).or_else(|| {
        let stripped = ["-minimal", "-low", "-medium", "-high", "-xhigh", "-extra-high", "-max"]
            .iter()
            .find_map(|s| target.strip_suffix(s))?;
        models.iter().find(|i| base_spec(&i.spec) == stripped)
    })
}

/// The levels a model offers, "default" first.
pub fn effort_levels(offered: Option<Vec<String>>) -> Vec<String> {
    match offered {
        Some(l) if !l.is_empty() => std::iter::once("default".to_string()).chain(l).collect(),
        Some(_) => vec!["default".to_string()],
        None => std::iter::once("default").chain(EFFORTS).map(String::from).collect(),
    }
}

/// The reasoning-effort list: an icon, the level and a check on the current one.
#[derive(IntoElement)]
pub struct EffortPicker {
    c: Colors,
    levels: Vec<String>,
    current: String,
    on_pick: Rc<dyn Fn(String, &mut Window, &mut App)>,
}

impl EffortPicker {
    pub fn new(c: Colors, levels: Vec<String>, current: String, on_pick: impl Fn(String, &mut Window, &mut App) + 'static) -> Self {
        Self { c, levels, current, on_pick: Rc::new(on_pick) }
    }
}

impl RenderOnce for EffortPicker {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let c = self.c;
        // Lowest at the bottom, next to the button the popover opens from.
        div().flex().flex_col().gap_0p5().children(self.levels.into_iter().enumerate().rev().map(|(i, level)| {
            let on = level == self.current;
            let pick = self.on_pick.clone();
            let l = level.clone();
            div().id(("effort", i)).flex().items_center().gap_2p5().h(px(34.0)).px_2p5().rounded_lg().cursor_pointer().text_size(px(13.0))
                .when(on, |d| d.bg(alpha(c.accent, 0.12)).text_color(c.fg))
                .when(!on, |d| d.text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                .on_click(move |_, window, cx| pick(l.clone(), window, cx))
                .child(effort_glyph(if on { c.accent } else { c.muted }, &level, 18.0))
                .child(div().flex_1().child(effort_label(&level)))
                .when(on, |d| d.child(Icon::new(IconName::Check).size(px(14.0)).text_color(c.accent)))
        }))
    }
}

/// Antigravity's multicolour arch, drawn as an image since `svg()` only tints in one colour.
static AGY_MARK: std::sync::LazyLock<std::sync::Arc<Image>> = std::sync::LazyLock::new(|| {
    std::sync::Arc::new(Image::from_bytes(ImageFormat::Svg, include_bytes!("../../assets/providers/agy-color.svg").to_vec()))
});

/// A provider's brand colour, tuned for legibility on the active theme.
pub(super) fn backend_color(c: Colors, backend: &str) -> Hsla {
    match backend {
        "claude" => c.claude,
        "chatgpt" | "codex" | "grok" => c.fg,
        "agy" => super::super::theme::hex(if c.light { "#3b78e7" } else { "#5b95ff" }),
        "opencode" => super::super::theme::hex(if c.light { "#c27803" } else { "#f5b544" }),
        "cursor" => super::super::theme::hex(if c.light { "#6366f1" } else { "#a0a0ff" }),
        "pi" => super::super::theme::hex(if c.light { "#16a34a" } else { "#7ee787" }),
        "muse" => super::super::theme::hex(if c.light { "#e11d48" } else { "#fb7185" }),
        "winfer" => c.accent,
        "shell" => c.muted,
        _ => super::super::theme::hex(if c.light { "#16a34a" } else { "#7ee787" }),
    }
}

/// A provider's brand mark in its colour.
pub(super) fn mark(c: Colors, backend: &str, size: f32, opacity: f32) -> AnyElement {
    if backend == "agy" {
        return img(AGY_MARK.clone()).size(px(size)).flex_shrink_0().opacity(opacity).into_any_element();
    }
    if backend == "shell" {
        return Icon::new(IconName::SquareTerminal).size(px(size)).flex_shrink_0().text_color(alpha(c.muted, opacity)).into_any_element();
    }
    let file = match backend {
        "claude" => "claude",
        "chatgpt" => "openai",
        "codex" => "codex",
        "agy" => "agy",
        "opencode" => "opencode",
        "cursor" => "cursor",
        "pi" => "pi",
        "muse" => "muse",
        "winfer" => "winfer",
        "grok" => "grok",
        _ => "local",
    };
    let color = backend_color(c, backend);
    svg().path(SharedString::from(format!("providers/{file}.svg"))).size(px(size)).flex_shrink_0().text_color(alpha(color, opacity)).into_any_element()
}

/// Accent pill marking winfer code as the recommended agent across settings and pickers.
pub(super) fn recommended_badge(c: Colors, compact: bool) -> Div {
    div()
        .h(px(if compact { 16.0 } else { 19.0 }))
        .px(px(if compact { 6.0 } else { 7.0 }))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(crate::theme::blend(c.panel, c.accent, if compact { 0.16 } else { 0.18 }))
        .border_1()
        .border_color(alpha(c.accent, if compact { 0.34 } else { 0.38 }))
        .text_size(px(if compact { 9.5 } else { 10.0 }))
        .font_weight(if compact { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD })
        .text_color(c.accent)
        .child("Recommended")
}

/// A mode's solid glyph in one colour: a question, a pencil, a list, a flag and a bolt.
pub fn mode_glyph(tint: Hsla, mode: Mode, size: f32) -> AnyElement {
    let file = match mode {
        Mode::Ask => "mode-ask",
        Mode::AutoEdit => "mode-edit",
        Mode::Plan => "mode-plan",
        Mode::YoloPlan => "mode-flag",
        Mode::Yolo => "mode-yolo",
    };
    svg().path(SharedString::from(format!("solid/{file}.svg"))).size(px(size)).flex_shrink_0().text_color(tint).into_any_element()
}

/// Picker state that lives while a popover is open.
#[derive(Default)]
pub struct PickerState {
    pub search: Option<Entity<InputState>>,
    /// The rail's selection: a backend id or the favourites star.
    pub tab: Option<String>,
    /// Keyboard-focused row index (from arrow keys or search); None lets the active model highlight alone.
    pub selected: Option<usize>,
    pub show_all: bool,
    /// The specs on screen, in order, for ⌘1–9 and the arrow keys.
    pub rows: Vec<String>,
    /// A click outside just closed this one: the button's own click must not reopen it.
    pub out_close: Option<(Sheet, Instant)>,
    /// Monotonic counter incremented on every Ultracode toggle to trigger fresh ignition animations.
    pub ultra_seq: u64,
    /// When Ultracode was last toggled and which direction (`true` = ignited on, `false` = turned off).
    pub ultra_flash: Option<(Instant, bool)>,
}

const ULTRA_IGNITE_MS: u64 = 1150;
const ULTRA_COOL_MS: u64 = 380;

fn ultra_noise(i: usize, salt: u64) -> f32 {
    let mut x = (i as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x % 10_000) as f32 / 10_000.0
}

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

fn ease_out_back(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.4 * u * u * u + 1.4 * u * u
}

/// Paints a sleek, tapered 4-point concave celestial starlight glint (`✦`) with a soft halo and
/// pinpoint core using quadratic Bézier curves pulled into the center.
fn paint_glint(window: &mut Window, cx: f32, cy: f32, r: f32, color: Hsla) {
    if r < 0.75 || color.a < 0.01 {
        return;
    }
    let halo = r * 2.3;
    window.paint_quad(
        fill(
            Bounds::new(point(px(cx - halo * 0.5), px(cy - halo * 0.5)), size(px(halo), px(halo))),
            alpha(color, (color.a * 0.20).clamp(0.0, 1.0)),
        )
        .corner_radii(Corners::all(px(halo * 0.5))),
    );
    let mut pb = PathBuilder::fill();
    let center = point(px(cx), px(cy));
    let rx = r * 0.84;
    let ry = r * 1.18;
    pb.move_to(point(px(cx), px(cy - ry)));
    pb.curve_to(point(px(cx + rx), px(cy)), center);
    pb.curve_to(point(px(cx), px(cy + ry)), center);
    pb.curve_to(point(px(cx - rx), px(cy)), center);
    pb.curve_to(point(px(cx), px(cy - ry)), center);
    pb.close();
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
    let core = (r * 0.28).clamp(1.0, 2.0);
    window.paint_quad(
        fill(
            Bounds::new(point(px(cx - core * 0.5), px(cy - core * 0.5)), size(px(core), px(core))),
            alpha(gpui_kit::white(), (color.a * 0.92).clamp(0.0, 1.0)),
        )
        .corner_radii(Corners::all(px(core * 0.5))),
    );
}

/// Paints a soft luminous stardust mote with a gentle outer glow.
fn paint_mote(window: &mut Window, cx: f32, cy: f32, sz: f32, color: Hsla) {
    if sz < 0.4 || color.a < 0.01 {
        return;
    }
    let halo = sz * 2.6;
    window.paint_quad(
        fill(
            Bounds::new(point(px(cx - halo * 0.5), px(cy - halo * 0.5)), size(px(halo), px(halo))),
            alpha(color, (color.a * 0.24).clamp(0.0, 1.0)),
        )
        .corner_radii(Corners::all(px(halo * 0.5))),
    );
    window.paint_quad(
        fill(Bounds::new(point(px(cx - sz * 0.5), px(cy - sz * 0.5)), size(px(sz), px(sz))), color)
            .corner_radii(Corners::all(px(sz * 0.5))),
    );
}

/// Computes `image_bounds` for `window.paint_image` so the `ULTRA_PAD`-texel atlas bleed border
/// sits strictly outside `bounds` while preserving `ObjectFit::Cover` aspect fill.
fn ultra_image_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let cover = ObjectFit::Cover.get_bounds(
        bounds,
        size(DevicePixels(nebula::ULTRA_INNER_W as i32), DevicePixels(nebula::ULTRA_INNER_H as i32)),
    );
    let pad_x = cover.size.width * (nebula::ULTRA_PAD as f32 / nebula::ULTRA_INNER_W as f32);
    let pad_y = cover.size.height * (nebula::ULTRA_PAD as f32 / nebula::ULTRA_INNER_H as f32);
    Bounds::new(
        point(cover.origin.x - pad_x, cover.origin.y - pad_y),
        size(cover.size.width + pad_x * 2.0, cover.size.height + pad_y * 2.0),
    )
}

#[cfg(test)]
thread_local! {
    static ULTRA_PAINT_LOG: std::cell::RefCell<Vec<(SharedString, usize, Bounds<Pixels>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn take_ultra_paint_log() -> Vec<(SharedString, usize, Bounds<Pixels>)> {
    ULTRA_PAINT_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

/// The Ultracode backdrop: a single opaque `window.paint_image` draw per frame from a pre-baked
/// 240-frame (6.0 s swirl, matching `nebula_send`) multi-frame `RenderImage`, wrapped in an
/// optional smooth bloom-in / dissolve-out opacity transition so the nebula wakes up gracefully
/// instead of popping in.
#[allow(clippy::too_many_arguments)]
fn ultra_aurora_layer(
    id: SharedString,
    light: bool,
    inset: Pixels,
    radius: Pixels,
    opacity: f32,
    ignite_seq: Option<u64>,
    cool_seq: Option<u64>,
    bloom_ms: u64,
) -> AnyElement {
    let r_f = f32::from(radius);
    let show_stars = opacity >= 0.45;
    #[cfg(test)]
    let id_log = id.clone();
    let fade_id_base = id.clone();
    let nebula = div()
        .absolute()
        .inset_0()
        .with_animation(id, Animation::new(Duration::from_millis(6000)).repeat_synced().with_max_fps(60.0), move |d, t| {
            #[cfg(test)]
            let id_p = id_log.clone();
            let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                if w <= 2.0 || h <= 2.0 {
                    return;
                }
                let seq = nebula::ultracode_sequence(light);
                let count = seq.frame_count().max(1);
                let frame_idx = ((t.rem_euclid(1.0) * count as f32) as usize) % count;
                #[cfg(test)]
                ULTRA_PAINT_LOG.with(|log| log.borrow_mut().push((id_p.clone(), frame_idx, bounds)));
                let _ = window.paint_image(
                    bounds,
                    ultra_image_bounds(bounds),
                    Corners::all(radius),
                    seq,
                    frame_idx,
                    false,
                );

                if !show_stars {
                    return;
                }
                // Subtle softly twinkling pinpoint stars strictly inside the rounded corners.
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let star_count = if w > 180.0 { 12usize } else { 5usize };
                let star_col = if light { theme::hex("#ffffff") } else { theme::hex("#e0e7ff") };
                let phi = t * std::f32::consts::TAU;
                for k in 0..star_count {
                    let sx = (0.08 + ultra_noise(k, 101) * 0.84) * w;
                    let sy = (0.16 + ultra_noise(k, 202) * 0.68) * h;
                    let dx = (r_f - sx).max(sx - (w - r_f)).max(0.0);
                    let dy = (r_f - sy).max(sy - (h - r_f)).max(0.0);
                    if dx * dx + dy * dy > (r_f - 2.0).max(0.0).powi(2) {
                        continue;
                    }
                    let phase = ultra_noise(k, 303) * std::f32::consts::TAU;
                    let wave = 0.5 + 0.5 * (phi + phase).sin();
                    let a = (0.12 + 0.55 * wave * wave) * if light { 0.85 } else { 0.72 };
                    let sz = 1.2 + ultra_noise(k, 404) * 1.1;
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(x0 + sx - sz * 0.5), px(y0 + sy - sz * 0.5)), size(px(sz), px(sz))),
                            alpha(star_col, a),
                        )
                        .corner_radii(Corners::all(px(sz * 0.5))),
                    );
                }
            };
            d.child(canvas(|_, _, _| {}, paint).size_full())
        });

    let wrap = div().absolute().inset(inset).child(nebula);
    if let Some(seq) = ignite_seq {
        wrap.with_animation(
            SharedString::from(format!("{fade_id_base}-in-{seq}")),
            Animation::new(Duration::from_millis(bloom_ms)),
            move |d, t| {
                // Smooth bloom-in with a subtle awakening crest that settles cleanly at `opacity`.
                let crest = (t * std::f32::consts::PI).sin().powi(2) * (1.0 - t) * 0.38;
                let a = (opacity * (ease_out_cubic(t) + crest)).clamp(0.0, 1.0);
                d.opacity(a)
            },
        )
        .into_any_element()
    } else if let Some(seq) = cool_seq {
        wrap.with_animation(
            SharedString::from(format!("{fade_id_base}-out-{seq}")),
            Animation::new(Duration::from_millis(ULTRA_COOL_MS)),
            move |d, t| d.opacity((opacity * (1.0 - ease_out_cubic(t))).clamp(0.0, 1.0)),
        )
        .into_any_element()
    } else {
        wrap.opacity(opacity).into_any_element()
    }
}

/// Soft, low-opacity Ultracode nebula wash spanning the entire composer card behind the textarea
/// and toolbar controls, blooming in smoothly on ignite and dissolving cleanly on cool-off.
pub(super) fn ultracode_composer_aurora(
    light: bool,
    radius: Pixels,
    ignite_seq: Option<u64>,
    cool_seq: Option<u64>,
) -> AnyElement {
    let opacity = if light { 0.24 } else { 0.25 };
    ultra_aurora_layer(
        SharedString::from("ultra-composer-aurora"),
        light,
        px(1.0),
        radius,
        opacity,
        ignite_seq,
        cool_seq,
        760,
    )
}

/// Unclipped weightless starlight glints & rising stardust motes emitted from the toggle switch
/// and card rim on ignition (graceful concave `✦` glints + buoyant embers, zero harsh confetti).
fn ultra_ignition_burst(id: SharedString, seq: u64, light: bool, radius_scale: f32, priority: usize) -> AnyElement {
    let palette = if light {
        [theme::hex("#6366f1"), theme::hex("#7c3aed"), theme::hex("#0284c7"), theme::hex("#c026d3")]
    } else {
        [theme::hex("#f8fafc"), theme::hex("#bae6fd"), theme::hex("#e9d5ff"), theme::hex("#c084fc")]
    };
    div()
        .absolute()
        .inset_0()
        .with_animation(id, Animation::new(Duration::from_millis(ULTRA_IGNITE_MS)), move |d, t| {
            if t >= 0.995 {
                return d;
            }
            let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));

                // 7 tapered concave `✦` starlight glints: concentrated around the switch thumb
                // (right side) with a few delicate accents along the upper rim, scaling smoothly
                // 0 -> peak -> 0 as they float upward.
                for k in 0..7usize {
                    let r = |salt: u64| ultra_noise(k, seq.wrapping_mul(41).wrapping_add(salt));
                    let born = k as f32 * 0.032 + r(1) * 0.05;
                    let life = 0.62 + r(2) * 0.22;
                    let p = (t - born) / life;
                    if !(0.0..1.0).contains(&p) {
                        continue;
                    }
                    let e = ease_out_cubic(p);
                    let bell = (p * std::f32::consts::PI).sin().powf(1.35);
                    let near_switch = k < 4;
                    let ox = if near_switch {
                        x0 + w * (0.76 + r(3) * 0.20)
                    } else {
                        x0 + w * (0.12 + r(3) * 0.60)
                    };
                    let oy = if near_switch {
                        y0 + h * (0.18 + r(4) * 0.64)
                    } else {
                        y0 + h * (0.04 + r(4) * 0.22)
                    };
                    let dx = (r(5) - 0.35) * 26.0 * radius_scale * e + (p * std::f32::consts::PI * 1.5 + r(6) * 6.0).sin() * 2.5 * p;
                    let dy = -(10.0 + r(7) * 24.0) * radius_scale * e;
                    let star_r = (3.0 + r(8) * 2.6) * radius_scale * (0.28 + 0.72 * bell);
                    let col = alpha(palette[k % palette.len()], (bell * if light { 0.88 } else { 0.96 }).clamp(0.0, 1.0));
                    paint_glint(window, ox + dx, oy + dy, star_r, col);
                }

                // 14 fine buoyant stardust motes rising gently from the toggle & upper edge.
                for k in 0..14usize {
                    let r = |salt: u64| ultra_noise(k + 30, seq.wrapping_mul(53).wrapping_add(salt));
                    let born = r(1) * 0.16;
                    let life = 0.56 + r(2) * 0.28;
                    let p = (t - born) / life;
                    if !(0.0..1.0).contains(&p) {
                        continue;
                    }
                    let e = ease_out_cubic(p);
                    let fade_in = (p / 0.16).min(1.0);
                    let fade_out = (1.0 - p).powf(1.45);
                    let a = fade_in * fade_out * if light { 0.78 } else { 0.86 };
                    let ox = if k < 8 {
                        x0 + w * (0.68 + r(3) * 0.28)
                    } else {
                        x0 + w * (0.10 + r(3) * 0.72)
                    };
                    let oy = y0 + h * (0.12 + r(4) * 0.72);
                    let sway = (p * std::f32::consts::PI * 1.6 + r(5) * std::f32::consts::TAU).sin() * 5.5 * p;
                    let dx = (r(6) - 0.35) * 22.0 * radius_scale * e + sway;
                    let dy = -(14.0 + r(7) * 32.0) * radius_scale * e;
                    let sz = (1.1 + r(8) * 1.2) * (1.0 - 0.45 * p);
                    let col = alpha(palette[(k + 1) % palette.len()], a.clamp(0.0, 1.0));
                    paint_mote(window, ox + dx, oy + dy, sz, col);
                }
            };
            d.child(deferred(div().absolute().inset_0().child(canvas(|_, _, _| {}, paint).size_full())).with_priority(priority))
        })
        .into_any_element()
}

/// Narrow chromatic light-blade that glides smoothly left-to-right inside the card without
/// overflowing the rounded corners or flooding the rest of the card.
fn ultra_sweep_blade(id: SharedString, light: bool, inset: Pixels, radius: Pixels) -> AnyElement {
    let lead = theme::hex("#38bdf8");
    let core = gpui_kit::white();
    let trail = theme::hex("#e879f9");
    let r_f = f32::from(radius);
    div()
        .absolute()
        .inset(inset)
        .with_animation(id, Animation::new(Duration::from_millis(620)), move |d, t| {
            if t >= 0.995 {
                return d;
            }
            let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let e = ease_out_cubic(t);
                let intensity = (1.0 - (t * 2.0 - 1.0).powi(2)).max(0.0);
                let a = intensity * if light { 0.45 } else { 0.52 };
                // Two adjacent gradient halves (transparent -> core -> transparent) clamped inside
                // the inner rounded rect so GPUI's 2-stop gradient never floods past the blade.
                let half = (w * 0.22).max(20.0);
                let inner_l = x0 + r_f * 0.45;
                let inner_r = x0 + w - r_f * 0.45;
                let cx = inner_l + (inner_r - inner_l) * e;
                let l0 = (cx - half).max(inner_l);
                let r1 = (cx + half).min(inner_r);
                let peak = alpha(core, a * 0.68);
                let edge_r = px((r_f * 0.65).max(2.0));
                if cx > l0 + 1.0 {
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(l0), px(y0)), size(px(cx - l0), px(h))),
                            linear_gradient(90.0, linear_color_stop(alpha(trail, 0.0), 0.0), linear_color_stop(peak, 1.0)),
                        )
                        .corner_radii(Corners { top_left: edge_r, bottom_left: edge_r, top_right: px(0.0), bottom_right: px(0.0) }),
                    );
                }
                if r1 > cx + 1.0 {
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(cx), px(y0)), size(px(r1 - cx), px(h))),
                            linear_gradient(90.0, linear_color_stop(peak, 0.0), linear_color_stop(alpha(lead, 0.0), 1.0)),
                        )
                        .corner_radii(Corners { top_left: px(0.0), bottom_left: px(0.0), top_right: edge_r, bottom_right: edge_r }),
                    );
                }
            };
            d.child(canvas(|_, _, _| {}, paint).size_full())
        })
        .into_any_element()
}

/// Coordinated composer ignition effect when Ultracode wakes up:
/// 1) a soft luminous glass sheen gliding left-to-right across the composer interior,
/// 2) a gentle rounded perimeter border glow pulse + top-edge prismatic light sweep, and
/// 3) a few whisper-subtle rising starlight glints & motes lifting off the composer's top edge.
pub(super) fn ultracode_composer_ignite(seq: u64, light: bool, inner_radius: Pixels) -> AnyElement {
    let cyan = theme::hex("#38bdf8");
    let violet = theme::hex("#a855f7");
    let orchid = theme::hex("#e879f9");
    let starlight = if light { theme::hex("#7c3aed") } else { theme::hex("#e0f2fe") };
    let r_f = f32::from(inner_radius);
    let sheen = div()
        .absolute()
        .inset(px(1.0))
        .with_animation(SharedString::from(format!("ultra-comp-ignite-{seq}")), Animation::new(Duration::from_millis(980)), move |d, t| {
            if t >= 0.995 {
                return d;
            }
            let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                if w <= 4.0 || h <= 4.0 {
                    return;
                }

                // 1. Soft glass light-veil sweeping left-to-right across the composer (first 68%).
                let sw_t = (t / 0.68).clamp(0.0, 1.0);
                if sw_t < 0.995 {
                    let e = ease_out_cubic(sw_t);
                    let env = (sw_t * std::f32::consts::PI).sin().powf(1.1);
                    let a = env * if light { 0.16 } else { 0.18 };
                    let half = (w * 0.24).max(36.0);
                    let inner_l = x0 + r_f * 0.55;
                    let inner_r = x0 + w - r_f * 0.55;
                    let cx = inner_l + (inner_r - inner_l) * e;
                    let l0 = (cx - half).max(inner_l);
                    let r1 = (cx + half).min(inner_r);
                    let peak = alpha(if light { violet } else { cyan }, a);
                    let edge_r = px((r_f * 0.6).max(4.0));
                    if cx > l0 + 1.0 {
                        window.paint_quad(
                            fill(
                                Bounds::new(point(px(l0), px(y0)), size(px(cx - l0), px(h))),
                                linear_gradient(90.0, linear_color_stop(alpha(orchid, 0.0), 0.0), linear_color_stop(peak, 1.0)),
                            )
                            .corner_radii(Corners { top_left: edge_r, bottom_left: edge_r, top_right: px(0.0), bottom_right: px(0.0) }),
                        );
                    }
                    if r1 > cx + 1.0 {
                        window.paint_quad(
                            fill(
                                Bounds::new(point(px(cx), px(y0)), size(px(r1 - cx), px(h))),
                                linear_gradient(90.0, linear_color_stop(peak, 0.0), linear_color_stop(alpha(cyan, 0.0), 1.0)),
                            )
                            .corner_radii(Corners { top_left: px(0.0), bottom_left: px(0.0), top_right: edge_r, bottom_right: edge_r }),
                        );
                    }
                }

                // 2. Soft rounded inner-perimeter awakening halo.
                let rim_env = (t * std::f32::consts::PI).sin().powf(1.2) * (1.0 - t * 0.35);
                if rim_env > 0.01 {
                    window.paint_quad(PaintQuad {
                        bounds,
                        corner_radii: Corners::all(inner_radius),
                        background: gpui_kit::transparent_black().into(),
                        border_widths: Edges::all(px(1.5)),
                        border_color: alpha(if light { violet } else { cyan }, rim_env * if light { 0.38 } else { 0.42 }),
                        border_style: BorderStyle::Solid,
                    });
                }

                // 3. Prismatic center-out light sheen along the top edge.
                let e = ease_out_cubic(t);
                let wave_fade = (1.0 - t).powf(1.3) * (t / 0.12).min(1.0) * if light { 0.72 } else { 0.88 };
                let span = ((w - r_f * 2.0) * (0.14 + 0.86 * e)).max(0.0);
                if span > 4.0 && wave_fade > 0.01 {
                    let left = x0 + (w - span) * 0.5;
                    let r = Corners::all(px(1.0));
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(left), px(y0)), size(px(span * 0.5), px(2.0))),
                            linear_gradient(90.0, linear_color_stop(alpha(violet, 0.0), 0.0), linear_color_stop(alpha(cyan, wave_fade), 1.0)),
                        )
                        .corner_radii(r),
                    );
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(left + span * 0.5), px(y0)), size(px(span * 0.5), px(2.0))),
                            linear_gradient(90.0, linear_color_stop(alpha(cyan, wave_fade), 0.0), linear_color_stop(alpha(orchid, 0.0), 1.0)),
                        )
                        .corner_radii(r),
                    );
                }
            };
            let motes_paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
                let (w, _) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                if w <= 20.0 {
                    return;
                }
                // 3 delicate concave `✦` glints + 7 micro stardust motes lifting lazily off the top edge.
                for k in 0..3usize {
                    let r = |salt: u64| ultra_noise(k + 70, seq.wrapping_mul(67).wrapping_add(salt));
                    let born = 0.08 + k as f32 * 0.08;
                    let life = 0.68;
                    let p = (t - born) / life;
                    if !(0.0..1.0).contains(&p) {
                        continue;
                    }
                    let e = ease_out_cubic(p);
                    let bell = (p * std::f32::consts::PI).sin().powf(1.4);
                    let ox = x0 + w * (0.18 + k as f32 * 0.30 + (r(1) - 0.5) * 0.14);
                    let dy = -(8.0 + r(2) * 16.0) * e;
                    let dx = (r(3) - 0.5) * 14.0 * e;
                    let sr = (2.6 + r(4) * 1.6) * (0.3 + 0.7 * bell);
                    paint_glint(window, ox + dx, y0 + dy, sr, alpha(starlight, (bell * 0.82).clamp(0.0, 1.0)));
                }
                for k in 0..7usize {
                    let r = |salt: u64| ultra_noise(k + 90, seq.wrapping_mul(73).wrapping_add(salt));
                    let born = 0.04 + r(1) * 0.18;
                    let life = 0.60 + r(2) * 0.22;
                    let p = (t - born) / life;
                    if !(0.0..1.0).contains(&p) {
                        continue;
                    }
                    let e = ease_out_cubic(p);
                    let a = (p / 0.16).min(1.0) * (1.0 - p).powf(1.5) * 0.72;
                    let ox = x0 + w * (0.10 + r(3) * 0.80);
                    let sway = (p * std::f32::consts::PI * 1.5 + r(4) * 6.0).sin() * 4.5 * p;
                    let dy = -(10.0 + r(5) * 22.0) * e;
                    let sz = (1.0 + r(6) * 1.1) * (1.0 - 0.4 * p);
                    let tint = if k % 2 == 0 { cyan } else { orchid };
                    paint_mote(window, ox + sway, y0 + dy, sz, alpha(tint, a.clamp(0.0, 1.0)));
                }
            };
            d.child(canvas(|_, _, _| {}, paint).size_full())
                .child(deferred(div().absolute().inset_0().child(canvas(|_, _, _| {}, motes_paint).size_full())).with_priority(12))
        });
    sheen.into_any_element()
}

impl CodeView {
    /// Opens a picker from its button, or closes it when it is the one open.
    pub(super) fn toggle_picker(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        if self.model.read(cx).state.sheet == Some(sheet) {
            self.set_sheet(None, cx);
            return;
        }
        if self.picker.out_close.is_some_and(|(s, at)| s == sheet && at.elapsed().as_millis() < 300) {
            return;
        }
        self.picker.tab = None;
        self.picker.selected = None;
        self.picker.show_all = false;
        self.set_sheet(Some(sheet), cx);
    }

    /// Creates and focuses the search field while the model picker is open; drops it after.
    pub(super) fn ensure_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.model.read(cx).state.sheet == Some(Sheet::Model);
        if open && self.picker.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let has_query = !input.read(cx).value().trim().is_empty();
                    this.picker.selected = has_query.then_some(0);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let idx = this.picker.selected.unwrap_or(0);
                    let spec = this.picker.rows.get(idx).cloned();
                    if let Some(spec) = spec {
                        this.pick_model(&spec, window, cx);
                    }
                }
                _ => {}
            })
            .detach();
            input.read(cx).focus_handle(cx).focus(window, cx);
            self.picker.search = Some(input);
        } else if !open && self.picker.search.is_some() {
            self.picker.search = None;
            self.focus_composer(window, cx);
        }
    }

    pub(super) fn pick_model(&mut self, spec: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let spec = spec.to_string();
        self.picker.selected = None;
        self.send(Cmd::SetModel(spec), cx);
        cx.notify();
    }

    pub(super) fn toggle_ultracode(&mut self, cx: &mut Context<Self>) {
        let next = !self.model.read(cx).state.ultracode;
        self.picker.ultra_seq = self.picker.ultra_seq.wrapping_add(1);
        self.picker.ultra_flash = Some((Instant::now(), next));
        self.send(Cmd::SetUltracode(next), cx);
        cx.notify();
    }

    pub(super) fn ultra_igniting(&self) -> Option<u64> {
        self.picker
            .ultra_flash
            .filter(|(at, on)| *on && at.elapsed() < Duration::from_millis(ULTRA_IGNITE_MS))
            .map(|_| self.picker.ultra_seq)
    }

    pub(super) fn ultra_cooling(&self) -> Option<u64> {
        self.picker
            .ultra_flash
            .filter(|(at, on)| !*on && at.elapsed() < Duration::from_millis(ULTRA_COOL_MS))
            .map(|_| self.picker.ultra_seq)
    }

    /// The Ultracode toggle card rendered inside the Effort picker (and Model picker's Reasoning
    /// column) whenever the active model is Claude Code or Codex.
    pub(super) fn ultracode_toggle(&self, c: Colors, backend: &str, on: bool, compact: bool, scope: &'static str, cx: &mut Context<Self>) -> AnyElement {
        nebula::warm_ultracode(c.light);
        let seq = self.picker.ultra_seq;
        let igniting = on && self.picker.ultra_flash.is_some_and(|(at, dir)| dir && at.elapsed() < Duration::from_millis(ULTRA_IGNITE_MS));
        let cooling = !on && self.picker.ultra_flash.is_some_and(|(at, dir)| !dir && at.elapsed() < Duration::from_millis(ULTRA_COOL_MS));
        let violet = theme::hex("#a855f7");
        let cyan = theme::hex("#38bdf8");
        let is_codex = backend == "codex";
        let subtitle = match (is_codex, compact) {
            (true, true) => "Multi-agent · Ultra",
            (true, false) => "Multi-agent v2 · Ultra effort",
            (false, true) => "Dynamic workflows",
            (false, false) => "Dynamic multi-agent workflows",
        };
        let title_col = if on {
            if c.light { theme::hex("#2e1065") } else { gpui_kit::white() }
        } else {
            c.fg
        };
        let sub_col = if on {
            if c.light { theme::hex("#5b21b6") } else { alpha(gpui_kit::white(), 0.78) }
        } else {
            c.subtle
        };
        let border_col = if on {
            if c.light { alpha(violet, 0.20) } else { alpha(gpui_kit::white(), 0.14) }
        } else {
            c.border
        };
        let hover_border_on = if c.light { alpha(violet, 0.30) } else { alpha(gpui_kit::white(), 0.24) };

        // Animated switch thumb (elastic overshoot on ignite, crisp slide on cool-off).
        // `switch` has a 1px border on all sides, so `absolute()` children are positioned inside
        // the `(track_w - 2) × (track_h - 2)` padding box; compute `pad` and `travel` from the
        // inner box so the thumb is evenly centered vertically and horizontally in both states.
        let (track_w, track_h, knob_sz) = if compact { (30.0, 18.0, 12.0) } else { (34.0, 20.0, 14.0) };
        let inner_w = track_w - 2.0;
        let inner_h = track_h - 2.0;
        let pad = (inner_h - knob_sz) * 0.5;
        let travel = inner_w - knob_sz - pad * 2.0;
        let knob_base = div()
            .debug_selector(move || format!("ultra-knob-{scope}"))
            .absolute()
            .top(px(pad))
            .size(px(knob_sz))
            .rounded_full()
            .bg(if on { gpui_kit::white() } else { c.muted })
            .shadow(vec![BoxShadow {
                color: if on { alpha(cyan, 0.55) } else { hsla(0.0, 0.0, 0.0, 0.28) },
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(if on { 6.0 } else { 3.0 }),
                spread_radius: px(0.0),
                inset: false,
            }]);
        let knob: AnyElement = if igniting {
            knob_base
                .with_animation(SharedString::from(format!("ultra-knob-on-{scope}-{seq}")), Animation::new(Duration::from_millis(420)), move |d, t| {
                    let x = (pad + travel * ease_out_back(t)).clamp(pad - 1.0, pad + travel + 1.0);
                    d.left(px(x))
                })
                .into_any_element()
        } else if cooling {
            knob_base
                .with_animation(SharedString::from(format!("ultra-knob-off-{scope}-{seq}")), Animation::new(Duration::from_millis(240)), move |d, t| {
                    let x = pad + travel * (1.0 - ease_out_cubic(t));
                    d.left(px(x))
                })
                .into_any_element()
        } else {
            knob_base.left(px(if on { pad + travel } else { pad })).into_any_element()
        };
        let switch = div()
            .debug_selector(move || format!("ultra-switch-{scope}"))
            .relative()
            .w(px(track_w))
            .h(px(track_h))
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(if on { alpha(cyan, 0.7) } else { c.border_strong })
            .bg(if on { alpha(violet, 0.85) } else { c.hover })
            .child(knob);

        let card = div()
            .id(SharedString::from(format!("pk-ultracode-{scope}")))
            .relative()
            .flex()
            .items_center()
            .gap(px(if compact { 8.0 } else { 10.0 }))
            .px(px(if compact { 8.0 } else { 10.0 }))
            .py(px(if compact { 7.0 } else { 9.0 }))
            .rounded(px(10.0))
            .border_1()
            .border_color(border_col)
            .cursor_pointer()
            .when(!on, |d| d.bg(alpha(c.hover, 0.55)).hover(move |d| d.bg(c.hover).border_color(alpha(violet, 0.24))))
            .when(on, |d| {
                d.shadow(vec![
                    BoxShadow {
                        color: alpha(violet, if c.light { 0.20 } else { 0.18 }),
                        offset: point(px(0.0), px(2.0)),
                        blur_radius: px(12.0),
                        spread_radius: px(0.0),
                        inset: false,
                    },
                    BoxShadow {
                        color: alpha(gpui_kit::white(), if c.light { 0.55 } else { 0.08 }),
                        offset: point(px(0.0), px(1.0)),
                        blur_radius: px(1.0),
                        spread_radius: px(0.0),
                        inset: true,
                    },
                ])
                .hover(move |d| d.border_color(hover_border_on))
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_ultracode(cx)))
            // Rounded nebula + sweeping light-blade background strictly inside the 1px border,
            // blooming smoothly in on ignite and dissolving cleanly out on cool-off.
            .when(on || cooling, |d| {
                let card_r = px(9.0);
                d.child(ultra_aurora_layer(
                    SharedString::from(format!("ultra-card-aurora-{scope}")),
                    c.light,
                    px(1.0),
                    card_r,
                    1.0,
                    igniting.then_some(seq),
                    cooling.then_some(seq),
                    420,
                ))
                .when(igniting, |d| d.child(ultra_sweep_blade(SharedString::from(format!("ultra-card-sweep-{scope}-{seq}")), c.light, px(1.0), card_r)))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(if compact { 12.0 } else { 13.0 }))
                            .line_height(px(if compact { 14.5 } else { 15.5 }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(title_col)
                            .child("Ultracode"),
                    )
                    .child(
                        div()
                            .text_size(px(if compact { 10.5 } else { 11.0 }))
                            .line_height(px(if compact { 13.0 } else { 13.5 }))
                            .text_color(sub_col)
                            .truncate()
                            .child(subtitle),
                    ),
            )
            .child(div().relative().child(switch))
            // Unclipped shockwave + starburst particles escaping the card and popover.
            .when(igniting, |d| d.child(ultra_ignition_burst(SharedString::from(format!("ultra-card-burst-{scope}-{seq}")), seq, c.light, if compact { 0.85 } else { 1.08 }, 15)));

        card.into_any_element()
    }

    /// ⌘1–9 in the model picker.
    pub(super) fn pick_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.picker.rows.get(n).cloned() {
            Some(spec) => {
                self.pick_model(&spec, window, cx);
                true
            }
            None => false,
        }
    }

    pub(super) fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let last = self.picker.rows.len().saturating_sub(1);
        let next = match self.picker.selected {
            Some(cur) => if down { (cur + 1).min(last) } else { cur.saturating_sub(1) },
            None => {
                let current = self.model.read(cx).state.model.clone();
                let models = self.model.read(cx).state.models.clone();
                let active_spec = match_model(&models, &current).map(|m| m.spec.as_str()).unwrap_or(current.as_str());
                let cur = self.picker.rows.iter().position(|r| r == active_spec).unwrap_or(0);
                if down { (cur + 1).min(last) } else { cur.saturating_sub(1) }
            }
        };
        self.picker.selected = Some(next);
        cx.notify();
    }

    fn favorites(&self, cx: &App) -> Vec<String> {
        self.app.read(cx).rest.get("codeWeb").and_then(|v| v["favModels"].as_array()).map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
    }

    fn toggle_favorite(&mut self, spec: &str, cx: &mut Context<Self>) {
        let mut favs = self.favorites(cx);
        match favs.iter().position(|f| f == spec) {
            Some(at) => {
                favs.remove(at);
            }
            None => favs.push(spec.to_string()),
        }
        self.app.update(cx, |s, cx| {
            let entry = s.rest.entry("codeWeb".to_string()).or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                *entry = serde_json::json!({});
            }
            entry["favModels"] = serde_json::json!(favs);
            cx.notify();
        });
        cx.notify();
    }

    /// Wraps a popover so it opens upward from the button it is a child of.
    fn popover(&mut self, sheet: Sheet, c: Colors, panel: Div, cx: &mut Context<Self>) -> AnyElement {
        let panel = panel.id("cw-popover").occlude().rounded_xl().border_1().border_color(c.edge).bg(c.raised).shadow_lg().overflow_hidden()
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                this.picker.out_close = Some((sheet, Instant::now()));
                this.set_sheet(None, cx);
            }));
        deferred(anchored().anchor(Anchor::BottomLeft).offset(point(px(0.0), px(-8.0))).snap_to_window_with_margin(px(8.0)).child(rise(panel, SharedString::from(format!("pop-{sheet:?}")), 0, 8.0))).with_priority(10).into_any_element()
    }

    pub(super) fn model_popover(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let (models, current, fav) = {
            let st = &self.model.read(cx).state;
            (st.models.clone(), st.model.clone(), self.favorites(cx))
        };
        let query = self.picker.search.as_ref().map(|i| i.read(cx).value().trim().to_lowercase()).unwrap_or_default();
        let listed: Vec<&ModelInfo> = models.iter().filter(|m| !is_fast(&m.spec)).collect();
        let backends: Vec<(&str, &str)> = BACKENDS.into_iter().filter(|(id, _)| listed.iter().any(|m| backend_of(&m.spec).map(|b| b.0) == Some(*id))).collect();
        let tab = self.picker.tab.clone().unwrap_or_else(|| backend_of(&current).map(|b| b.0.to_string()).or_else(|| backends.first().map(|b| b.0.to_string())).unwrap_or_else(|| FAVORITES.to_string()));
        let mut rows: Vec<&ModelInfo> = listed.iter().copied().filter(|m| {
            if !query.is_empty() {
                m.label.to_lowercase().contains(&query) || m.spec.to_lowercase().contains(&query)
            } else if tab == FAVORITES {
                fav.contains(&m.spec)
            } else {
                backend_of(&m.spec).map(|b| b.0) == Some(tab.as_str())
            }
        }).collect();
        let total = rows.len();
        let capped = !self.picker.show_all && query.is_empty() && total > SHOWN;
        if capped {
            rows.truncate(SHOWN);
        }
        self.picker.rows = rows.iter().map(|m| m.spec.clone()).collect();
        let selected_row = self.picker.selected.map(|i| i.min(rows.len().saturating_sub(1)));

        let mut rail = div().w(px(54.0)).flex_shrink_0().flex().flex_col().items_center().gap_1p5().py_2p5();
        let mut tabs: Vec<String> = vec![FAVORITES.to_string()];
        tabs.extend(backends.iter().map(|(id, _)| id.to_string()));
        for (i, id) in tabs.into_iter().enumerate() {
            let on = tab == id && query.is_empty();
            let pick = id.clone();
            rail = rail.child(div().id(("pk-tab", i)).relative().size(px(34.0)).flex().items_center().justify_center().rounded_lg().cursor_pointer()
                .when(on, |d| d.bg(c.hover)).hover(move |d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker.tab = Some(pick.clone());
                    this.picker.selected = None;
                    this.picker.show_all = false;
                    cx.notify();
                }))
                .when(on, |d| d.child(div().absolute().right(px(-10.0)).top(px(8.0)).w(px(2.0)).h(px(18.0)).rounded_full().bg(c.accent)))
                .child(if id == FAVORITES { star_icon(if on { c.warning } else { alpha(c.warning, 0.65) }, 17.0) } else { mark(c, &id, 18.0, if on { 1.0 } else { 0.6 }) }));
        }

        let mut list = div().id("pk-list").flex_1().min_h_0().overflow_y_scroll().p_2().flex().flex_col().gap_0p5();
        if rows.is_empty() {
            let text = if !query.is_empty() { "No model matches" } else if tab == FAVORITES { "Star a model to keep it here" } else { "No models listed yet. They appear once the session has started." };
            list = list.child(div().px_3().py_4().text_size(px(12.0)).text_color(c.subtle).child(text));
        }
        for (i, info) in rows.iter().enumerate() {
            let spec = info.spec.clone();
            let name = if info.label.is_empty() { model_name(&spec, None) } else { info.label.clone() };
            let on = base_spec(&current) == spec || match_model(&models, &current).is_some_and(|m| m.spec == spec);
            let twin = fast_spec(&spec, &models);
            let on_fast = twin.as_deref() == Some(current.as_str());
            let active = on || on_fast;
            let is_fav = fav.contains(&spec);
            let backend = backend_of(&spec).map_or("", |b| b.0);
            let tier = model_tier(&spec, &info.detail);
            let group = SharedString::from(format!("pk-row-{i}"));
            let (pick, star, zap) = (spec.clone(), spec.clone(), twin.clone().map(|t| if on_fast { spec.clone() } else { t }));
            list = list.child(div().id(("pk-row", i)).group(group.clone()).h(px(36.0)).flex().items_center().gap_2p5().px_2p5().rounded_lg().cursor_pointer().text_size(px(13.0)).text_color(c.fg)
                .when(active, |d| d.bg(alpha(c.accent, 0.12)))
                .when(!active && selected_row == Some(i), |d| d.bg(c.hover))
                .hover(move |d| if active { d.bg(alpha(c.accent, 0.16)) } else { d.bg(c.hover) })
                .on_click(cx.listener(move |this, _, window, cx| this.pick_model(&pick, window, cx)))
                .child(mark(c, backend, 16.0, 1.0))
                .child(div().min_w_0().truncate().child(name))
                .when(active, |d| d.child(Icon::new(IconName::Check).size(px(13.0)).text_color(c.accent)))
                .when(matches!(tier, ModelTier::Fast), |d| d.child(div().text_size(px(10.0)).px_1p5().rounded_full().bg(alpha(c.warning, 0.14)).text_color(c.warning).child("light")))
                .child(div().flex_1())
                .when_some(zap, |d, z| d.child(div().id(("pk-fast", i)).size(px(24.0)).flex().items_center().justify_center().rounded_md().hover(move |d| d.bg(c.active))
                    .tooltip(super::code_view::tip("Fast mode"))
                    .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.pick_model(&z, window, cx); }))
                    .child(Icon::new(IconName::Zap).size(px(13.0)).text_color(if on_fast { c.warning } else { c.subtle }))))
                .child(div().id(("pk-star", i)).size(px(24.0)).flex().items_center().justify_center().rounded_md().hover(move |d| d.bg(c.active))
                    .when(!is_fav, |d| d.opacity(0.0).group_hover(group.clone(), |d| d.opacity(1.0)))
                    .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.toggle_favorite(&star, cx); }))
                    .child(star_icon(if is_fav { c.warning } else { c.subtle }, 13.0)))
                .when(i < 9, |d| d.child(div().min_w(px(30.0)).h(px(20.0)).flex().items_center().justify_center().rounded_md().bg(c.hover).text_size(px(11.0)).text_color(c.subtle).child(format!("⌘{}", i + 1)))));
        }
        if total > SHOWN && query.is_empty() {
            let all = self.picker.show_all;
            list = list.child(div().id("pk-more").px_2p5().py_1p5().cursor_pointer().text_size(px(12.0)).text_color(c.subtle).hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(|this, _, _, cx| { this.picker.show_all = !this.picker.show_all; cx.notify(); }))
                .child(if all { "Show less".to_string() } else { format!("Show {} more", total - SHOWN) }));
        }

        let search = div().flex().items_center().gap_2().h(px(44.0)).px_3()
            .child(Icon::new(IconName::Search).size(px(15.0)).text_color(c.subtle))
            .child(div().flex_1().when_some(self.picker.search.clone(), |d, input| d.child(Input::new(&input).appearance(false))));

        let st = &self.model.read(cx).state;
        let offered = match_model(&models, &st.model).and_then(|i| i.efforts.clone());
        let levels = effort_levels(offered);
        let effort = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
        let model_label = if st.model.is_empty() { String::new() } else { model_name(&st.model, st.actual_model.as_deref()) };
        let ultra_backend = supports_ultracode(&st.model).then(|| backend_of(&st.model).map(|b| b.0)).flatten();
        let ultra_on = st.ultracode;
        let view = cx.entity();
        let ultra_section = ultra_backend.map(|b| {
            div()
                .p_2()
                .border_t_1()
                .border_color(c.edge)
                .child(self.ultracode_toggle(c, b, ultra_on, true, "model", cx))
        });
        let reasoning = div().w(px(196.0)).flex_shrink_0().flex().flex_col()
            .child(div().flex().items_center().justify_between().h(px(44.0)).px_3().text_size(px(12.0)).text_color(c.subtle).child("Reasoning").child(div().truncate().child(model_label)))
            .child(div().id("pk-effort").flex_1().min_h_0().overflow_y_scroll().p_2().child(EffortPicker::new(c, levels, effort, move |level, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.send(Cmd::SetEffort(if level == "default" { None } else { Some(level) }), cx);
                    this.set_sheet(None, cx);
                });
            })))
            .children(ultra_section);

        let panel = div().w(px(580.0)).h(px(372.0)).flex()
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_selection(false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_selection(true, cx)))
            .child(rail)
            .child(div().flex_1().min_w_0().flex().flex_col().child(search).child(list))
            .child(reasoning);
        self.popover(Sheet::Model, c, panel, cx)
    }

    pub(super) fn effort_popover(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let (offered, effort, model_label, ultra_backend, ultra_on) = {
            let st = &self.model.read(cx).state;
            let offered = match_model(&st.models, &st.model).and_then(|i| i.efforts.clone());
            let effort = if st.effort.is_empty() { "default".to_string() } else { st.effort.clone() };
            let model_label = if st.model.is_empty() { String::new() } else { model_name(&st.model, st.actual_model.as_deref()) };
            let ultra_backend = supports_ultracode(&st.model).then(|| backend_of(&st.model).map(|b| b.0)).flatten();
            (offered, effort, model_label, ultra_backend, st.ultracode)
        };
        let levels = effort_levels(offered);
        let hint = if ultra_on && ultra_backend == Some("codex") {
            "Ultracode boosts Codex to Ultra reasoning + multi-agent v2"
        } else if ultra_on && ultra_backend == Some("claude") {
            "Ultracode runs dynamic multi-agent workflows alongside your effort"
        } else {
            effort_hint(&effort)
        };
        let ultra_section = ultra_backend.map(|b| {
            div()
                .px_2()
                .pb_2()
                .pt_2()
                .border_t_1()
                .border_color(c.edge)
                .child(self.ultracode_toggle(c, b, ultra_on, false, "effort", cx))
        });
        let view = cx.entity();
        let codex_boosted = ultra_on && ultra_backend == Some("codex");
        let effort_list = div()
            .p_2()
            .when(codex_boosted, |d| d.opacity(0.55))
            .child(EffortPicker::new(c, levels, effort, move |level, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.send(Cmd::SetEffort(if level == "default" { None } else { Some(level) }), cx);
                    this.set_sheet(None, cx);
                });
            }));
        let foot = div()
            .px_3()
            .pb_2p5()
            .pt_0p5()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(div().flex_1().min_w_0().text_size(px(11.5)).text_color(c.subtle).child(hint))
            .when(ultra_backend.is_some(), |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .px_1p5()
                        .py(px(1.0))
                        .rounded_md()
                        .bg(c.hover)
                        .border_1()
                        .border_color(c.border)
                        .text_size(px(10.0))
                        .text_color(c.subtle)
                        .child("Tab"),
                )
            });
        let panel = div().w(px(292.0)).flex().flex_col()
            .child(div().flex().items_center().justify_between().h(px(40.0)).px_3().text_size(px(12.0)).text_color(c.subtle).child("Reasoning").child(div().truncate().child(model_label)))
            .child(effort_list)
            .children(ultra_section)
            .child(foot);
        self.popover(Sheet::Effort, c, panel, cx)
    }

    pub(super) fn mode_popover(&mut self, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let current = self.model.read(cx).state.mode;
        let mut list = div().p_2().flex().flex_col().gap_0p5();
        for (i, mode) in Mode::ALL.into_iter().enumerate() {
            let on = mode == current;
            list = list.child(div().id(("pk-mode", i)).flex().items_center().gap_2p5().px_2p5().py_1p5().rounded_lg().cursor_pointer()
                .when(on, |d| d.bg(alpha(c.accent, 0.12)))
                .when(!on, |d| d.hover(move |d| d.bg(c.hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(Cmd::SetMode(mode), cx);
                    this.set_sheet(None, cx);
                }))
                .child(mode_glyph(if on { c.accent } else { c.muted }, mode, 18.0))
                .child(div().flex_1().flex().flex_col().child(div().text_size(px(13.0)).text_color(c.fg).child(mode.label())).child(div().text_size(px(11.5)).text_color(c.subtle).child(mode.hint())))
                .when(on, |d| d.child(Icon::new(IconName::Check).size(px(14.0)).text_color(c.accent))));
        }
        let panel = div().w(px(330.0)).flex().flex_col().child(list).child(div().px_3().pb_2().text_size(px(11.0)).text_color(c.subtle).child("⇧⇥ cycles the mode"));
        self.popover(Sheet::Mode, c, panel, cx)
    }
}
