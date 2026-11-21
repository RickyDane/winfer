//! Session names change on their own (smart naming): the old name turns to dust, swept left to
//! right, each letter crumbling into specks that blow up and away, then the new name fades in.

use super::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The old name crumbling, specks included.
const DUST_MS: u64 = 1500;
/// The whole swap; the new name fades in over its second half.
const TOTAL_MS: u64 = 1700;
/// Specks per letter.
const SPECKS: usize = 22;
/// Share of the dust animation the left-to-right sweep takes to reach the last letter.
const SWEEP: f32 = 0.42;
/// How long a letter takes to crumble once the sweep reaches it.
const CRUMBLE: f32 = 0.16;

struct Swap {
    shown: String,
    prev: Option<String>,
    at: Instant,
    seq: u64,
}

static SWAPS: Mutex<Option<HashMap<String, Swap>>> = Mutex::new(None);
/// Names smart naming just gave, by pane: only these changes animate (not the user's renames).
static ARMED: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);
/// How long a smart name waits for its pane to be drawn with it.
const ARMED_MS: u64 = 3000;

/// Smart naming named `pane` `title`: the next time its name changes to that, animate it.
pub(super) fn arm(pane: &str, title: &str) {
    let mut guard = ARMED.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    map.retain(|_, (_, at)| at.elapsed() < Duration::from_millis(ARMED_MS));
    map.insert(pane.to_string(), (title.to_string(), Instant::now()));
}

fn armed(pane: &str, label: &str) -> bool {
    ARMED.lock().as_ref().and_then(|m| m.get(pane)).is_some_and(|(t, at)| t == label && at.elapsed() < Duration::from_millis(ARMED_MS))
}

/// Cheap deterministic pseudo-random in 0..1.
fn noise(a: usize, b: usize, salt: u64) -> f32 {
    let mut x = ((a as u64) << 32 | b as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x % 10_000) as f32 / 10_000.0
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// When the sweep reaches letter `i` of `n` (0..1 of the dust animation).
fn letter_start(i: usize, n: usize, seq: u64) -> f32 {
    0.04 + SWEEP * i as f32 / (n.max(2) - 1) as f32 + noise(i, 999, seq) * 0.04
}

/// The specks one letter breaks into, painted over everything (not clipped by the title's
/// truncation) so they can drift out of the line.
fn specks(i: usize, start: f32, seq: u64, t: f32) -> [AnyElement; 2] {
    // Deferred drawing loses the inherited text style, so the letter's colour is read in place
    // (an empty canvas in the letter) and handed to the specks, painted later.
    let tint = std::rc::Rc::new(std::cell::Cell::new(None::<Hsla>));
    let probe = {
        let tint = tint.clone();
        canvas(move |_, window, _| tint.set(Some(window.text_style().color)), |_, _, _, _| {}).absolute().size_0()
    };
    let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
        let color = tint.get().unwrap_or_else(|| window.text_style().color);
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        for k in 0..SPECKS {
            let r = |salt: u64| noise(i, k, seq.wrapping_mul(31).wrapping_add(salt));
            // Specks leave from the letter's left edge first, following the sweep.
            let (ox, oy) = (r(1), 0.2 + r(2) * 0.65);
            let born = start + ox * CRUMBLE + r(3) * 0.05;
            let life = 0.38 + r(4) * 0.4;
            let p = (t - born) / life;
            if !(0.0..1.0).contains(&p) {
                continue;
            }
            let e = ease_out(p);
            // Blown up and to the right, wavering a little, some caught by a stronger gust.
            let gust = if r(5) > 0.8 { 1.8 } else { 1.0 };
            let dx = (10.0 + r(6) * 34.0) * gust * e;
            let dy = -(4.0 + r(7) * 18.0) * gust * e + (p * 9.0 + r(8) * 6.0).sin() * 1.4 * p;
            let sz = (0.8 + r(9) * 1.5) * (1.0 - 0.55 * p);
            let a = color.a * (1.0 - p).powf(1.4) * (0.55 + r(10) * 0.45);
            let (x, y) = (x0 + ox * w + dx, y0 + oy * h + dy);
            window.paint_quad(fill(Bounds::new(point(px(x - sz / 2.0), px(y - sz / 2.0)), size(px(sz), px(sz))), Hsla { a, ..color }));
        }
    };
    [
        probe.into_any_element(),
        deferred(div().absolute().top_0().left_0().size_full().child(canvas(|_, _, _| {}, paint).size_full())).with_priority(5).into_any_element(),
    ]
}

/// A pane's name where it's shown (`surface`: the sidebar row, the header): plain text, or the
/// swap animation when smart naming just changed it.
pub(super) fn title(surface: &str, pane: &str, label: &str) -> AnyElement {
    let key = &format!("{surface}-{pane}");
    let (prev, seq) = {
        let mut guard = SWAPS.lock();
        let map = guard.get_or_insert_with(HashMap::new);
        match map.get_mut(key) {
            Some(s) if s.shown != label => {
                let old = std::mem::replace(&mut s.shown, label.to_string());
                s.prev = armed(pane, label).then_some(old);
                s.at = Instant::now();
                s.seq += 1;
                (s.prev.clone(), s.seq)
            }
            Some(s) => (s.prev.clone().filter(|_| s.at.elapsed() < Duration::from_millis(TOTAL_MS)), s.seq),
            None => {
                map.insert(key.to_string(), Swap { shown: label.to_string(), prev: None, at: Instant::now(), seq: 0 });
                (None, 0)
            }
        }
    };
    let Some(prev) = prev else { return div().child(label.to_string()).into_any_element() };
    let new = div().relative().child(label.to_string()).with_animation(
        SharedString::from(format!("title-in-{key}-{seq}")),
        Animation::new(Duration::from_millis(TOTAL_MS)),
        |d, t| {
            let e = ease_out(((t - 0.5) / 0.4).clamp(0.0, 1.0));
            d.opacity(e).top(px(3.0 * (1.0 - e)))
        },
    );
    let chars: Vec<char> = prev.chars().take(60).collect();
    let n = chars.len();
    let old = div().absolute().top_0().left_0().flex().whitespace_nowrap().children(chars.into_iter().enumerate().map(move |(i, ch)| {
        let start = letter_start(i, n, seq);
        let blank = ch.is_whitespace();
        let text = if blank { '\u{a0}'.to_string() } else { ch.to_string() };
        div().relative().child(text).with_animation(
            SharedString::from(format!("title-out-{key}-{seq}-{i}")),
            Animation::new(Duration::from_millis(DUST_MS)),
            move |d, t| {
                // The letter thins out and slips a hair along with its dust, flickering as it goes.
                let l = ((t - start) / CRUMBLE).clamp(0.0, 1.0);
                let flicker = if l > 0.0 && l < 1.0 { 0.75 + 0.25 * (t * 90.0 + i as f32).sin().abs() } else { 1.0 };
                let d = d.opacity((1.0 - l).powf(1.6) * flicker).left(px(2.5 * l)).top(px(-1.0 * l));
                if blank || t < start || t >= 1.0 { d } else { d.children(specks(i, start, seq, t)) }
            },
        )
    }));
    div().relative().child(new).child(old).into_any_element()
}
