//! A procedurally painted nebula (domain-warped fractal noise in a few dye colours, with stars and a
//! dark rim), used as the send button's face. GPUI can't blur, so the clouds are painted into a
//! small image once and shown as that.

use gpui_kit::{Hsla, Image, ImageFormat, RenderImage, Rgba};
use parking_lot::Mutex;
use smallvec::smallvec;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ready,
    Idle,
    Stop,
}

const SIZE: u32 = 96;

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (xi, yi) = (xi as i32, yi as i32);
    let a = hash(xi, yi, seed);
    let b = hash(xi + 1, yi, seed);
    let c = hash(xi, yi + 1, seed);
    let d = hash(xi + 1, yi + 1, seed);
    (a + (b - a) * sx) + ((c + (d - c) * sx) - (a + (b - a) * sx)) * sy
}

fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for o in 0..5 {
        sum += amp * value_noise(x * freq, y * freq, seed + o);
        amp *= 0.5;
        freq *= 2.03;
    }
    sum
}

fn smooth(lo: f32, hi: f32, v: f32) -> f32 {
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hexc(s: &str) -> [f32; 3] {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    [((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// The RGBA pixels of a nebula, its clouds drifted to `phase` (radians); a full turn returns to the start.
/// Dark themes paint additive dye clouds on deep space with a dark rim; light themes wash luminous
/// pigments onto an airy base so the button doesn't sit like a black hole on a light composer.
fn paint_rect(w: u32, h: u32, kind: Kind, light: bool, phase: f32) -> Vec<u8> {
    let (ox, oy) = (phase.cos() * 0.55, phase.sin() * 0.55);
    let (bg, dye1, dye2, dye3, strength) = match (kind, light) {
        (Kind::Ready, false) => (hexc("#07030f"), hexc("#5b21b6"), hexc("#d946ef"), hexc("#38bdf8"), 1.0),
        (Kind::Stop, false) => (hexc("#0f0403"), hexc("#9a3412"), hexc("#e11d48"), hexc("#fbbf24"), 1.0),
        (Kind::Idle, false) => (hexc("#07050f"), hexc("#3b2a7a"), hexc("#5b2a86"), hexc("#1d4e89"), 0.55),
        (Kind::Ready, true) => (hexc("#6366f1"), hexc("#8b5cf6"), hexc("#e879f9"), hexc("#38bdf8"), 1.0),
        (Kind::Stop, true) => (hexc("#e11d48"), hexc("#f97316"), hexc("#fb7185"), hexc("#fde047"), 1.0),
        (Kind::Idle, true) => (hexc("#eef0f8"), hexc("#b4c0f8"), hexc("#dfc4fb"), hexc("#bae6fd"), 0.65),
    };
    let ar = w as f32 / h as f32;
    let half_spine = ((ar - 1.0) * 0.5).max(0.0);
    let star_thresh = if ar > 1.5 { 0.9978 } else { 0.9965 };
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for py in 0..h {
        for px in 0..w {
            let (u, v) = (px as f32 / h as f32, py as f32 / h as f32);
            // Warp the lookup with another noise field: that's what makes the clouds swirl and melt together.
            let (wx, wy) = (fbm(u * 2.6 + 3.1 + ox, v * 2.6 + oy, 11), fbm(u * 2.6 - oy, v * 2.6 + 7.7 + ox, 12));
            let (x, y) = (u * 2.8 + wx * 1.7, v * 2.8 + wy * 1.7);
            let d1 = fbm(x, y, 1);
            let d2 = fbm(x + 5.2, y + 1.3, 2);
            let d3 = fbm(x - 3.7, y + 8.1, 3);
            let (a1, a2, a3) = (smooth(0.28, 0.78, d1), smooth(0.4, 0.85, d2) * 0.9, smooth(0.5, 0.9, d3).powi(2) * 0.8);
            let core = smooth(0.55, 1.0, (d1 + d2) * 0.5);
            let (dx, dy) = (((u - ar * 0.5).abs() - half_spine).max(0.0), v - 0.5);
            let r = (dx * dx + dy * dy).sqrt() * 2.0;
            let star = hash(px as i32, py as i32, 99);
            let twinkle = if star > star_thresh { 0.6 + 0.4 * hash(px as i32, py as i32, 7) } else { 0.0 };

            let mut rgb = if light {
                let mut c = mix(bg, dye1, (a1 * 0.75 * strength).clamp(0.0, 1.0));
                c = mix(c, dye2, (a2 * 0.70 * strength).clamp(0.0, 1.0));
                c = mix(c, dye3, (a3 * 0.62 * strength).clamp(0.0, 1.0));
                let lift = core * if kind == Kind::Idle { 0.05 } else { 0.12 };
                for ch in c.iter_mut() {
                    *ch += lift;
                }
                let rim_depth = if kind == Kind::Idle { 0.09 } else { 0.20 };
                let rim = 1.0 - smooth(0.55, 1.05, r) * rim_depth;
                let star_k = if kind == Kind::Idle { 0.18 } else { 0.55 };
                for ch in c.iter_mut() {
                    *ch = (*ch * rim + twinkle * star_k).clamp(0.0, 1.0);
                }
                c
            } else {
                let mut c = bg;
                for k in 0..3 {
                    c[k] += (dye1[k] * a1 + dye2[k] * a2 + dye3[k] * a3) * strength * 0.75;
                }
                let lift = core * 0.12 * strength;
                for ch in c.iter_mut() {
                    *ch += lift;
                }
                let rim = 1.0 - smooth(0.55, 1.05, r) * 0.82;
                for ch in c.iter_mut() {
                    *ch = (*ch * rim + twinkle * if kind == Kind::Idle { 0.5 } else { 1.0 }).clamp(0.0, 1.0).powf(0.92);
                }
                c
            };
            for ch in rgb.iter_mut() {
                *ch = ch.clamp(0.0, 1.0);
            }
            out.extend_from_slice(&[(rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8, 255]);
        }
    }
    out
}

#[cfg(test)]
fn paint(kind: Kind, light: bool, phase: f32) -> Vec<u8> {
    paint_rect(SIZE, SIZE, kind, light, phase)
}

fn png_rect(w: u32, h: u32, kind: Kind, light: bool, phase: f32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let buf = image::RgbaImage::from_raw(w, h, paint_rect(w, h, kind, light, phase)).expect("buffer matches size");
    image::DynamicImage::ImageRgba8(buf).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png encodes");
    bytes
}

fn png(kind: Kind, light: bool, phase: f32) -> Vec<u8> {
    png_rect(SIZE, SIZE, kind, light, phase)
}

/// The nebula for a button state, painted on first use.
pub fn image(kind: Kind, light: bool) -> Arc<Image> {
    static DARK: OnceLock<[Arc<Image>; 3]> = OnceLock::new();
    static LIGHT: OnceLock<[Arc<Image>; 3]> = OnceLock::new();
    let cache = if light { &LIGHT } else { &DARK };
    let all = cache.get_or_init(|| [Kind::Ready, Kind::Idle, Kind::Stop].map(|k| Arc::new(Image::from_bytes(ImageFormat::Png, png(k, light, 0.0)))));
    match kind {
        Kind::Ready => all[0].clone(),
        Kind::Idle => all[1].clone(),
        Kind::Stop => all[2].clone(),
    }
}

// 36 painted frames over the 6 s loop; the button fades each into the next, so 60 fps stays smooth.
const FRAMES: usize = 36;

/// The looping drift of the working (ember) nebula, painted on first use: shown in turn while a session is working.
pub fn working_frames(light: bool) -> &'static [Arc<Image>] {
    static DARK: OnceLock<Vec<Arc<Image>>> = OnceLock::new();
    static LIGHT: OnceLock<Vec<Arc<Image>>> = OnceLock::new();
    let cache = if light { &LIGHT } else { &DARK };
    cache.get_or_init(|| (0..FRAMES).map(|i| Arc::new(Image::from_bytes(ImageFormat::Png, png(Kind::Stop, light, i as f32 / FRAMES as f32 * std::f32::consts::TAU)))).collect())
}
pub const ULTRA_INNER_W: u32 = 96;
pub const ULTRA_INNER_H: u32 = 32;
/// Extra texel border around each Ultracode nebula frame so `window.paint_image` can crop strictly
/// inside the atlas tile on all four edges (preventing bilinear bleed from neighboring atlas tiles)
/// while clipping all four corners with GPU SDF `corner_radii`.
pub const ULTRA_PAD: u32 = 1;
const ULTRA_W: u32 = ULTRA_INNER_W + ULTRA_PAD * 2;
const ULTRA_H: u32 = ULTRA_INNER_H + ULTRA_PAD * 2;
/// 240 unique frames over a 6.0 s loop (40 unique fps, matching the send button's 6 s swirl
/// tempo) with zero cross-fading and seamless loop continuity. All 240 tiles (`98×34`) fit inside
/// one 1024×1024 Metal atlas texture (`10 × 30 = 300` slots).
pub const ULTRA_FRAMES: usize = 240;

fn fbm4(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for o in 0..4 {
        sum += amp * value_noise(x * freq, y * freq, seed + o);
        amp *= 0.5;
        freq *= 2.03;
    }
    sum
}

/// Paints one opaque BGRA `image::Frame` of the Ultracode nebula at swirl `phase` (`0..TAU`),
/// using the same domain-warped FBM cloud synthesis and dye harmony as the composer send button
/// (`paint_rect` / `Kind::Ready`), tuned for wide surfaces (composer card & picker toggle).
fn paint_ultracode_frame_raw(light: bool, phase: f32) -> image::Frame {
    let (ox, oy) = (phase.cos() * 0.55, phase.sin() * 0.55);
    let (bg, dye1, dye2, dye3, strength) = if light {
        (hexc("#ece7fe"), hexc("#8b5cf6"), hexc("#e879f9"), hexc("#38bdf8"), 0.88)
    } else {
        (hexc("#0e0a18"), hexc("#5b21b6"), hexc("#d946ef"), hexc("#38bdf8"), 0.52)
    };
    let ar = ULTRA_INNER_W as f32 / ULTRA_INNER_H as f32;
    let half_spine = ((ar - 1.0) * 0.5).max(0.0);
    let inv_h = 1.0 / ULTRA_INNER_H as f32;
    let pad = ULTRA_PAD as f32;
    let mut out = Vec::with_capacity((ULTRA_W * ULTRA_H * 4) as usize);
    for py in 0..ULTRA_H {
        let v = (py as f32 - pad) * inv_h;
        for px in 0..ULTRA_W {
            let u = (px as f32 - pad) * inv_h;
            // Same domain-warped FBM swirl as the send button's nebula (`paint_rect`).
            let (wx, wy) = (
                fbm4(u * 2.6 + 3.1 + ox, v * 2.6 + oy, 11),
                fbm4(u * 2.6 - oy, v * 2.6 + 7.7 + ox, 12),
            );
            let (x, y) = (u * 2.8 + wx * 1.7, v * 2.8 + wy * 1.7);
            let d1 = fbm4(x, y, 1);
            let d2 = fbm4(x + 5.2, y + 1.3, 2);
            let d3 = fbm4(x - 3.7, y + 8.1, 3);
            let (a1, a2, a3) = (
                smooth(0.28, 0.78, d1),
                smooth(0.40, 0.85, d2) * 0.9,
                smooth(0.50, 0.90, d3).powi(2) * 0.8,
            );
            let core = smooth(0.55, 1.0, (d1 + d2) * 0.5);
            let (dx, dy) = (((u - ar * 0.5).abs() - half_spine).max(0.0), v - 0.5);
            let r = (dx * dx + dy * dy).sqrt() * 2.0;

            let rgb = if light {
                let mut c = mix(bg, dye1, (a1 * 0.72 * strength).clamp(0.0, 1.0));
                c = mix(c, dye2, (a2 * 0.66 * strength).clamp(0.0, 1.0));
                c = mix(c, dye3, (a3 * 0.60 * strength).clamp(0.0, 1.0));
                let lift = core * 0.08;
                let rim = 1.0 - smooth(0.60, 1.08, r) * 0.10;
                [
                    ((c[0] + lift) * rim).clamp(0.0, 1.0),
                    ((c[1] + lift) * rim).clamp(0.0, 1.0),
                    ((c[2] + lift) * rim).clamp(0.0, 1.0),
                ]
            } else {
                let mut c = bg;
                for k in 0..3 {
                    c[k] += (dye1[k] * a1 + dye2[k] * a2 + dye3[k] * a3) * strength * 0.75 + core * 0.10 * strength;
                }
                let rim = 1.0 - smooth(0.62, 1.08, r) * 0.30;
                [
                    (c[0] * rim).clamp(0.0, 1.0).powf(0.94),
                    (c[1] * rim).clamp(0.0, 1.0).powf(0.94),
                    (c[2] * rim).clamp(0.0, 1.0).powf(0.94),
                ]
            };
            // RenderImage expects BGRA byte order.
            out.extend_from_slice(&[(rgb[2] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[0] * 255.0) as u8, 255]);
        }
    }
    let buf = image::RgbaImage::from_raw(ULTRA_W, ULTRA_H, out).expect("ultracode buffer matches size");
    image::Frame::new(buf)
}

struct UltraLoopBank {
    first: OnceLock<Arc<RenderImage>>,
    sequence: OnceLock<Arc<RenderImage>>,
    warming: std::sync::atomic::AtomicBool,
}

impl UltraLoopBank {
    const fn new() -> Self {
        Self {
            first: OnceLock::new(),
            sequence: OnceLock::new(),
            warming: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn warm(&'static self, light: bool) {
        if self.sequence.get().is_some() || self.warming.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        std::thread::spawn(move || {
            let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
            let chunk = ULTRA_FRAMES.div_ceil(workers);
            let parts: Vec<Vec<image::Frame>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..workers)
                    .map(|w| {
                        let start = w * chunk;
                        let end = (start + chunk).min(ULTRA_FRAMES);
                        s.spawn(move || {
                            (start..end)
                                .map(|i| paint_ultracode_frame_raw(light, i as f32 / ULTRA_FRAMES as f32 * std::f32::consts::TAU))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().expect("ultracode frame worker")).collect()
            });
            let mut frames = smallvec::SmallVec::with_capacity(ULTRA_FRAMES);
            for part in parts {
                frames.extend(part);
            }
            let _ = self.sequence.set(Arc::new(RenderImage::new(frames)));
        });
    }

    fn get(&'static self, light: bool) -> Arc<RenderImage> {
        self.warm(light);
        if let Some(seq) = self.sequence.get() {
            return seq.clone();
        }
        self.first
            .get_or_init(|| Arc::new(RenderImage::new(smallvec![paint_ultracode_frame_raw(light, 0.0)])))
            .clone()
    }
}

static ULTRA_DARK: UltraLoopBank = UltraLoopBank::new();
static ULTRA_LIGHT: UltraLoopBank = UltraLoopBank::new();

/// Pre-warms the 240-frame Ultracode aurora `RenderImage` on a background thread so toggling
/// Ultracode on starts at full 60 fps with zero main-thread stall.
pub fn warm_ultracode(light: bool) {
    if light {
        ULTRA_LIGHT.warm(true);
    } else {
        ULTRA_DARK.warm(false);
    }
}

/// Returns the multi-frame `RenderImage` for the Ultracode aurora (240 frames once warm, or a
/// 1-frame immediate fallback while the background bake finishes in a few milliseconds).
pub fn ultracode_sequence(light: bool) -> Arc<RenderImage> {
    if light {
        ULTRA_LIGHT.get(true)
    } else {
        ULTRA_DARK.get(false)
    }
}

pub const SKY_INNER_W: u32 = 256;
pub const SKY_INNER_H: u32 = 160;
/// Extra texel border around the painted sky so `window.paint_image` can crop strictly inside the
/// atlas tile on all four edges (preventing bilinear bleed from neighboring atlas tiles) while
/// still clipping all four corners with GPU SDF `corner_radii`.
pub const SKY_PAD: u32 = 2;
const SKY_W: u32 = SKY_INNER_W + SKY_PAD * 2;
const SKY_H: u32 = SKY_INNER_H + SKY_PAD * 2;

fn paint_sky_bgra(light: bool, accent: Hsla) -> Vec<u8> {
    let mut out = Vec::with_capacity((SKY_W * SKY_H * 4) as usize);
    let radial = |u: f32, v: f32, cx: f32, cy: f32, rx: f32, ry: f32| -> f32 {
        let dx = (u - cx) / rx;
        let dy = (v - cy) / ry;
        let d = (dx * dx + dy * dy).sqrt();
        (1.0 - smooth(0.0, 0.72, d)).powf(1.35)
    };
    let (ca, sa) = (25_f32.to_radians().cos(), 25_f32.to_radians().sin());
    let acc = Rgba::from(accent);
    for py in 0..SKY_H {
        for px in 0..SKY_W {
            let (u, v) = (
                (px as f32 - SKY_PAD as f32) / SKY_INNER_W as f32,
                (py as f32 - SKY_PAD as f32) / SKY_INNER_H as f32,
            );
            let band = ((u - 0.5) * ca + (v - 0.5) * sa) / 0.17;
            let (r, g, b, a) = if light {
                let n1 = radial(u, v, 0.18, 0.12, 0.40, 0.30) * 0.07;
                let n2 = radial(u, v, 0.85, 0.88, 0.35, 0.28) * 0.05;
                let n3 = radial(u, v, 0.50, 1.15, 0.60, 0.35) * 0.05;
                let mw = (-band * band * 2.4).exp() * 0.10;
                let acc_w = n1 + n2 + mw;
                let r = acc.r * acc_w + (70.0 / 255.0) * n3;
                let g = acc.g * acc_w + (110.0 / 255.0) * n3;
                let b = acc.b * acc_w + (220.0 / 255.0) * n3;
                (r, g, b, (acc_w + n3).clamp(0.0, 0.42))
            } else {
                // Three radial nebula clouds matching `.cw-nebula`.
                let n1 = radial(u, v, 0.18, 0.12, 0.45, 0.32) * 0.03;
                let n2 = radial(u, v, 0.85, 0.88, 0.40, 0.30) * 0.027;
                let n3 = radial(u, v, 0.50, 1.15, 0.65, 0.38) * 0.03;
                // Soft diagonal Milky Way band along 115deg.
                let mw = (-band * band * 2.4).exp() * 0.025;
                let r = (88.0 / 255.0) * n1 + (124.0 / 255.0) * n2 + (40.0 / 255.0) * n3 + (195.0 / 255.0) * mw;
                let g = (80.0 / 255.0) * n1 + (140.0 / 255.0) * n2 + (70.0 / 255.0) * n3 + (202.0 / 255.0) * mw;
                let b = (220.0 / 255.0) * n1 + (255.0 / 255.0) * n2 + (160.0 / 255.0) * n3 + (255.0 / 255.0) * mw;
                (r, g, b, (n1 + n2 + n3 + mw).clamp(0.0, 0.42))
            };
            let (sr, sg, sb) = if a > 1e-4 {
                ((r / a).clamp(0.0, 1.0), (g / a).clamp(0.0, 1.0), (b / a).clamp(0.0, 1.0))
            } else {
                (0.0, 0.0, 0.0)
            };
            // RenderImage expects BGRA byte order.
            out.extend_from_slice(&[(sb * 255.0) as u8, (sg * 255.0) as u8, (sr * 255.0) as u8, (a * 255.0) as u8]);
        }
    }
    out
}

fn build_sky_render_image(light: bool, accent: Hsla) -> Arc<RenderImage> {
    let buf = image::RgbaImage::from_raw(SKY_W, SKY_H, paint_sky_bgra(light, accent)).expect("sky buffer matches size");
    Arc::new(RenderImage::new(smallvec![image::Frame::new(buf)]))
}

/// Soft transparent nebula + Milky Way glow painted behind the winfer code welcome screen
/// (matches `.cw-nebula` and `.cw-milkyway` in `src/features/code/code.css`), ready for
/// `window.paint_image` with rounded corners and 2-texel atlas bleed padding.
pub fn sky_backdrop_render(light: bool, accent: Hsla) -> Arc<RenderImage> {
    if !light {
        static DARK_SKY: OnceLock<Arc<RenderImage>> = OnceLock::new();
        return DARK_SKY.get_or_init(|| build_sky_render_image(false, accent)).clone();
    }
    static LIGHT_SKY: OnceLock<Mutex<Option<([u8; 3], Arc<RenderImage>)>>> = OnceLock::new();
    let acc = Rgba::from(accent);
    let key = [(acc.r * 255.0) as u8, (acc.g * 255.0) as u8, (acc.b * 255.0) as u8];
    let mut guard = LIGHT_SKY.get_or_init(|| Mutex::new(None)).lock();
    if let Some((k, img)) = guard.as_ref() {
        if *k == key {
            return img.clone();
        }
    }
    let img = build_sky_render_image(true, accent);
    *guard = Some((key, img.clone()));
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_a_dark_rim_and_a_lit_middle() {
        let p = paint(Kind::Ready, false, 0.0);
        assert_eq!(p.len(), (SIZE * SIZE * 4) as usize);
        let lum = |x: u32, y: u32| {
            let i = ((y * SIZE + x) * 4) as usize;
            p[i] as u32 + p[i + 1] as u32 + p[i + 2] as u32
        };
        let rim = lum(1, 1) + lum(SIZE - 2, 1) + lum(1, SIZE - 2) + lum(SIZE - 2, SIZE - 2);
        let mid: u32 = (40..56).flat_map(|y| (40..56).map(move |x| (x, y))).map(|(x, y)| lum(x, y)).sum::<u32>() / 256 * 4;
        assert!(mid > rim, "{mid} vs {rim}");
    }

    #[test]
    fn light_mode_is_visibly_lighter_in_every_state() {
        let mean_lum = |p: &[u8]| -> u32 {
            let sum: u64 = p.chunks_exact(4).map(|px| px[0] as u64 + px[1] as u64 + px[2] as u64).sum();
            (sum / (SIZE as u64 * SIZE as u64 * 3)) as u32
        };
        for kind in [Kind::Idle, Kind::Ready, Kind::Stop] {
            let dark = mean_lum(&paint(kind, false, 0.0));
            let light = mean_lum(&paint(kind, true, 0.0));
            assert!(light > dark + 40, "light ({light}) should be clearly brighter than dark ({dark})");
        }
        assert!(mean_lum(&paint(Kind::Idle, true, 0.0)) > 190);
    }

    #[test]
    fn encodes_png() {
        assert_eq!(&png(Kind::Stop, false, 1.0)[1..4], b"PNG");
        assert_eq!(&png(Kind::Stop, true, 1.0)[1..4], b"PNG");
    }

    #[test]
    fn ultracode_frames_flow_smoothly_and_loop_seamlessly() {
        let f0 = paint_ultracode_frame_raw(false, 0.0);
        let f_mid = paint_ultracode_frame_raw(false, std::f32::consts::PI);
        let f_last = paint_ultracode_frame_raw(false, (ULTRA_FRAMES - 1) as f32 / ULTRA_FRAMES as f32 * std::f32::consts::TAU);
        let f_light = paint_ultracode_frame_raw(true, 0.0);

        let mean_diff = |a: &image::Frame, b: &image::Frame| -> f32 {
            let (ba, bb) = (a.buffer().as_raw(), b.buffer().as_raw());
            let sum: u64 = ba.iter().zip(bb.iter()).map(|(&x, &y)| x.abs_diff(y) as u64).sum();
            sum as f32 / ba.len() as f32
        };
        let mean_lum = |f: &image::Frame| -> f32 {
            let b = f.buffer().as_raw();
            let sum: u64 = b.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum();
            sum as f32 / (b.len() as f32 / 4.0 * 3.0)
        };

        let mid_diff = mean_diff(&f0, &f_mid);
        let wrap_diff = mean_diff(&f0, &f_last);
        assert!(mid_diff > 4.0, "aurora clouds must visibly move across the loop, got mean_diff={mid_diff:.2}");
        assert!(wrap_diff < 1.5, "loop wrap from last frame to frame 0 must be seamless, got wrap_diff={wrap_diff:.2}");
        assert!(
            mean_lum(&f_light) > mean_lum(&f0) + 120.0,
            "light mode aurora ({:.1}) must be clearly brighter than dark mode ({:.1})",
            mean_lum(&f_light),
            mean_lum(&f0)
        );
    }
}
