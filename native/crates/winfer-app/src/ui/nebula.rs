//! A procedurally painted nebula (domain-warped fractal noise in a few dye colours, with stars and a
//! dark rim), used as the send button's face. GPUI can't blur, so the clouds are painted into a
//! small image once and shown as that.

use gpui_kit::{Image, ImageFormat};
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

/// The RGBA pixels of a nebula, its clouds drifted to `phase` (radians); a full turn returns to the start.
fn paint(kind: Kind, phase: f32) -> Vec<u8> {
    let (ox, oy) = (phase.cos() * 0.55, phase.sin() * 0.55);
    let (bg, dye1, dye2, dye3, strength) = match kind {
        Kind::Ready => (hexc("#07030f"), hexc("#5b21b6"), hexc("#d946ef"), hexc("#38bdf8"), 1.0),
        Kind::Stop => (hexc("#0f0403"), hexc("#9a3412"), hexc("#e11d48"), hexc("#fbbf24"), 1.0),
        Kind::Idle => (hexc("#07050f"), hexc("#3b2a7a"), hexc("#5b2a86"), hexc("#1d4e89"), 0.55),
    };
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (u, v) = (px as f32 / SIZE as f32, py as f32 / SIZE as f32);
            // Warp the lookup with another noise field: that's what makes the clouds swirl and melt together.
            let (wx, wy) = (fbm(u * 2.6 + 3.1 + ox, v * 2.6 + oy, 11), fbm(u * 2.6 - oy, v * 2.6 + 7.7 + ox, 12));
            let (x, y) = (u * 2.8 + wx * 1.7, v * 2.8 + wy * 1.7);
            let d1 = fbm(x, y, 1);
            let d2 = fbm(x + 5.2, y + 1.3, 2);
            let d3 = fbm(x - 3.7, y + 8.1, 3);
            let (a1, a2, a3) = (smooth(0.28, 0.78, d1), smooth(0.4, 0.85, d2) * 0.9, smooth(0.5, 0.9, d3).powi(2) * 0.8);
            let mut rgb = bg;
            for k in 0..3 {
                rgb[k] += (dye1[k] * a1 + dye2[k] * a2 + dye3[k] * a3) * strength;
            }
            // A brighter core where the dust is thickest.
            let core = smooth(0.55, 1.0, (d1 + d2) * 0.5) * 0.35 * strength;
            for c in rgb.iter_mut() {
                *c += core;
            }
            // Dark rim: deep space at the edge, the glow in the middle.
            let (dx, dy) = (u - 0.5, v - 0.5);
            let r = (dx * dx + dy * dy).sqrt() * 2.0;
            let rim = 1.0 - smooth(0.55, 1.05, r) * 0.82;
            // Stars.
            let star = hash(px as i32, py as i32, 99);
            let twinkle = if star > 0.9965 { 0.6 + 0.4 * hash(px as i32, py as i32, 7) } else { 0.0 };
            for c in rgb.iter_mut() {
                *c = (*c * rim + twinkle * if kind == Kind::Idle { 0.5 } else { 1.0 }).clamp(0.0, 1.0).powf(0.92);
            }
            out.extend_from_slice(&[(rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8, 255]);
        }
    }
    out
}

fn png(kind: Kind, phase: f32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let buf = image::RgbaImage::from_raw(SIZE, SIZE, paint(kind, phase)).expect("buffer matches size");
    image::DynamicImage::ImageRgba8(buf).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png encodes");
    bytes
}

/// The nebula for a button state, painted on first use.
pub fn image(kind: Kind) -> Arc<Image> {
    static CACHE: OnceLock<[Arc<Image>; 3]> = OnceLock::new();
    let all = CACHE.get_or_init(|| [Kind::Ready, Kind::Idle, Kind::Stop].map(|k| Arc::new(Image::from_bytes(ImageFormat::Png, png(k, 0.0)))));
    match kind {
        Kind::Ready => all[0].clone(),
        Kind::Idle => all[1].clone(),
        Kind::Stop => all[2].clone(),
    }
}

// 36 painted frames over the 6 s loop; the button fades each into the next, so 60 fps stays smooth.
const FRAMES: usize = 36;

/// The looping drift of the working (ember) nebula, painted on first use: shown in turn while a session is working.
pub fn working_frames() -> &'static [Arc<Image>] {
    static CACHE: OnceLock<Vec<Arc<Image>>> = OnceLock::new();
    CACHE.get_or_init(|| (0..FRAMES).map(|i| Arc::new(Image::from_bytes(ImageFormat::Png, png(Kind::Stop, i as f32 / FRAMES as f32 * std::f32::consts::TAU)))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_a_dark_rim_and_a_lit_middle() {
        let p = paint(Kind::Ready, 0.0);
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
    fn encodes_png() {
        assert_eq!(&png(Kind::Stop, 1.0)[1..4], b"PNG");
    }
}
