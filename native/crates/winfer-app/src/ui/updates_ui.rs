//! Update checks and installs, the "What's new" dialog and Settings → Updates (port of
//! `features/updates/updater.ts`, `WhatsNew.tsx` and the update footer of `SettingsModal.tsx`).

use super::commands::{Command, Toast, ToastKind};
use super::*;
use crate::backend;
use crate::updates::{self, Channel, ReleaseChanges};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

#[derive(Clone)]
pub(super) struct Available {
    pub version: String,
    /// This platform's signed bundle.
    pub bundle: updates::Bundle,
    /// The release page, to download it by hand when installing fails.
    pub page: String,
    pub notes: Vec<ReleaseChanges>,
}

/// An install's download so far: bytes received and the total (0 while unknown).
#[derive(Default)]
pub(super) struct Progress {
    got: AtomicU64,
    total: AtomicU64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Notes {
    Available,
    Installed,
}

pub(super) struct UpdatesUi {
    pub(super) version: String,
    channel: Channel,
    checking: bool,
    error: Option<String>,
    pub(super) available: Option<Available>,
    installed: Option<(String, Vec<ReleaseChanges>)>,
    open: Option<Notes>,
    offered: HashSet<String>,
    started: bool,
    /// Set while an update downloads and installs.
    installing: Option<Arc<Progress>>,
    /// When the last check finished without an error (ms since the epoch).
    checked_at: Option<f64>,
    /// Incremented on status transitions so resting hero transitions animate once.
    status_seq: u64,
}

impl Default for UpdatesUi {
    fn default() -> Self {
        Self { version: env!("CARGO_PKG_VERSION").to_string(), channel: Channel::Stable, checking: false, error: None, available: None, installed: None, open: None, offered: HashSet::new(), started: false, installing: None, checked_at: None, status_seq: 0 }
    }
}

impl UpdatesUi {
    pub(super) fn is_checking(&self) -> bool {
        self.checking
    }

    pub(super) fn is_installing(&self) -> bool {
        self.installing.is_some()
    }

    /// Download percentage (`0..=100`) while installing, once total size is known.
    pub(super) fn install_percent(&self) -> Option<u64> {
        let p = self.installing.as_ref()?;
        let total = p.total.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        let got = p.got.load(Ordering::Relaxed).min(total);
        Some(got * 100 / total)
    }

    /// Development aid: the dialog with made-up notes.
    #[cfg(debug_assertions)]
    pub(super) fn preview(&mut self) {
        let release = |v: &str, c: &[&str]| ReleaseChanges { version: v.into(), changes: c.iter().map(|s| s.to_string()).collect() };
        self.installed = Some((self.version.clone(), vec![release("0.2.0", &["Browser: phone view and page handles", "chat pane for winfer code", "fix crash on start"]), release("0.1.9", &["Viewer: PDFs"])]));
        self.open = Some(Notes::Installed);
    }

    /// Development aid: Settings → Updates in the "checking" animation state.
    #[cfg(debug_assertions)]
    pub(super) fn preview_checking(&mut self) {
        self.checking = true;
        self.available = None;
        self.installing = None;
        self.error = None;
        self.status_seq += 1;
    }

    /// Development aid: Settings → Updates in the "downloading / installing" animation state.
    #[cfg(debug_assertions)]
    pub(super) fn preview_installing(&mut self, got: u64, total: u64) {
        let progress = Arc::new(Progress::default());
        progress.got.store(got, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
        self.checking = false;
        self.error = None;
        self.available = Some(Available {
            version: "0.2.0".into(),
            bundle: updates::Bundle { url: String::new(), signature: String::new() },
            page: String::new(),
            notes: vec![ReleaseChanges { version: "0.2.0".into(), changes: vec!["Updates: smooth progress ring and shimmer while downloading".into()] }],
        });
        self.installing = Some(progress);
        self.status_seq += 1;
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

fn breathe(t: f32) -> f32 {
    0.5 + 0.5 * (t * std::f32::consts::TAU).sin()
}

pub(super) fn spinning_icon(icon: IconName, size: f32, color: Option<Hsla>, id: impl Into<ElementId>) -> AnimationElement<Icon> {
    let mut i = Icon::new(icon).size(px(size));
    if let Some(color) = color {
        i = i.text_color(color);
    }
    i.with_animation(id, Animation::new(Duration::from_millis(900)).repeat().with_max_fps(60.0), |i, t| i.rotate(radians(t * std::f32::consts::TAU)))
}

fn paint_arc(window: &mut Window, center: Point<Pixels>, radius: Pixels, stroke: f32, from: f32, to: f32, color: Hsla) {
    let steps = (((to - from).abs() * 64.0).ceil() as usize).max(2);
    let mut path = PathBuilder::stroke(px(stroke));
    for i in 0..=steps {
        let frac = from + (to - from) * i as f32 / steps as f32;
        let angle = frac * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let p = point(center.x + radius * angle.cos(), center.y + radius * angle.sin());
        if i == 0 { path.move_to(p) } else { path.line_to(p) }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Paints a smooth 3-stage orbiting comet arc on a circular track.
fn paint_orbit_arc(window: &mut Window, center: Point<Pixels>, radius: Pixels, stroke: f32, phase: f32, color: Hsla) {
    paint_arc(window, center, radius, stroke, phase, phase + 0.14, Hsla { a: color.a * 0.28, ..color });
    paint_arc(window, center, radius, stroke, phase + 0.10, phase + 0.25, Hsla { a: color.a * 0.62, ..color });
    paint_arc(window, center, radius, stroke, phase + 0.20, phase + 0.34, color);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallPhase {
    Starting,
    Downloading,
    Finalizing,
}

fn install_phase(got: u64, total: u64) -> InstallPhase {
    if total == 0 {
        InstallPhase::Starting
    } else if got >= total {
        InstallPhase::Finalizing
    } else {
        InstallPhase::Downloading
    }
}

/// Animated 40px status tile for the Updates hero card:
/// - Installing: circular progress ring (or orbiting arc) with floating Download / breathing Sparkles icon.
/// - Checking: subtle orbiting arc around a smoothly spinning RefreshCw icon.
/// - Resting: crisp icon tile with a one-shot settle animation on status transitions.
fn update_hero_tile(tint: Hsla, checking: bool, progress: Option<(u64, u64)>, resting_icon: IconName, status_seq: u64) -> AnyElement {
    use super::settings_kit::{alpha, icon_tile};
    const SIZE: f32 = 40.0;
    const RADIUS: f32 = SIZE * 0.27;

    if let Some((got, total)) = progress {
        let phase = install_phase(got, total);
        let share = if total > 0 { (got as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
        let ring = div().absolute().inset_0().with_animation(
            match phase {
                InstallPhase::Starting => "update-tile-ring-start",
                InstallPhase::Downloading => "update-tile-ring-dl",
                InstallPhase::Finalizing => "update-tile-ring-fin",
            },
            Animation::new(Duration::from_millis(if phase == InstallPhase::Downloading { 1600 } else { 1100 })).repeat().with_max_fps(60.0),
            move |d, t| {
                d.child(canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    let center = bounds.center();
                    let radius = px(15.0);
                    let stroke = 2.2;
                    paint_arc(window, center, radius, stroke, 0.0, 1.0, alpha(tint, 0.18));
                    if phase == InstallPhase::Downloading {
                        let sweep = share.max(0.04);
                        paint_arc(window, center, radius, stroke, 0.0, sweep, tint);
                        // Bright pulse gliding along the completed portion of the ring.
                        let glint_len = (sweep * 0.28).min(0.14);
                        let glint_start = (t * sweep).min((sweep - glint_len).max(0.0));
                        let glint_a = 0.55 * (t * std::f32::consts::PI).sin();
                        if glint_a > 0.02 {
                            paint_arc(window, center, radius, stroke + 0.4, glint_start, (glint_start + glint_len).min(sweep), hsla(0.0, 0.0, 1.0, glint_a));
                        }
                        // Soft pulsing dot at the leading edge of the progress arc.
                        let tip_angle = sweep * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                        let tip = point(center.x + radius * tip_angle.cos(), center.y + radius * tip_angle.sin());
                        let halo = 5.0 + 1.6 * breathe(t);
                        window.paint_quad(
                            fill(Bounds::new(point(tip.x - px(halo * 0.5), tip.y - px(halo * 0.5)), size(px(halo), px(halo))), alpha(tint, 0.35 + 0.3 * breathe(t)))
                                .corner_radii(Corners::all(px(halo * 0.5))),
                        );
                    } else {
                        paint_orbit_arc(window, center, radius, stroke, t, tint);
                    }
                }).size_full())
            },
        );
        let center_icon: AnyElement = if phase == InstallPhase::Finalizing {
            Icon::new(IconName::Sparkles).size(px(15.0)).text_color(tint)
                .with_animation("update-tile-fin-icon", Animation::new(Duration::from_millis(1200)).repeat().with_max_fps(30.0), |i, t| i.opacity(0.6 + 0.4 * breathe(t)))
                .into_any_element()
        } else {
            div().flex().items_center().justify_center().child(Icon::new(IconName::Download).size(px(15.0)).text_color(tint))
                .with_animation("update-tile-dl-bob", Animation::new(Duration::from_millis(1300)).repeat().with_max_fps(60.0), |d, t| {
                    d.relative().top(px((t * std::f32::consts::TAU).sin() * 1.5))
                })
                .into_any_element()
        };
        return div().relative().size(px(SIZE)).flex_shrink_0().flex().items_center().justify_center().rounded(px(RADIUS)).overflow_hidden()
            .child(ring)
            .child(center_icon)
            .with_animation("update-tile-bg-install", Animation::new(Duration::from_millis(1800)).repeat().with_max_fps(30.0), move |d, t| {
                d.bg(alpha(tint, 0.12 + 0.08 * breathe(t)))
            })
            .into_any_element();
    }

    if checking {
        let ring = div().absolute().inset_0().with_animation(
            "update-tile-ring-check",
            Animation::new(Duration::from_millis(1200)).repeat().with_max_fps(60.0),
            move |d, t| {
                d.child(canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    let center = bounds.center();
                    let radius = px(15.5);
                    paint_arc(window, center, radius, 1.8, 0.0, 1.0, alpha(tint, 0.14));
                    paint_orbit_arc(window, center, radius, 1.8, t, alpha(tint, 0.72));
                }).size_full())
            },
        );
        return div().relative().size(px(SIZE)).flex_shrink_0().flex().items_center().justify_center().rounded(px(RADIUS)).overflow_hidden()
            .child(ring)
            .child(spinning_icon(IconName::RefreshCw, 16.0, Some(tint), "update-tile-check-spin"))
            .with_animation("update-tile-bg-check", Animation::new(Duration::from_millis(1600)).repeat().with_max_fps(30.0), move |d, t| {
                d.bg(alpha(tint, 0.11 + 0.07 * breathe(t)))
            })
            .into_any_element();
    }

    let tile = icon_tile(tint, resting_icon, SIZE);
    if status_seq > 0 {
        tile.with_animation(
            SharedString::from(format!("update-tile-settle-{status_seq}")),
            Animation::new(Duration::from_millis(260)),
            |d, t| {
                let e = ease_out(t);
                d.opacity(0.45 + 0.55 * e).relative().top(px(2.5 * (1.0 - e)))
            },
        )
        .into_any_element()
    } else {
        tile.into_any_element()
    }
}

/// Subtle sweeping accent ribbon along the top inner edge of the Updates hero card while active.
fn update_hero_sweep(tint: Hsla, installing: bool) -> AnyElement {
    let dur = if installing { 1900 } else { 2300 };
    let peak = if installing { 0.65 } else { 0.42 };
    div().absolute().top_0().left_0().right_0().h(px(2.0)).overflow_hidden()
        .with_animation(
            if installing { "update-hero-sweep-install" } else { "update-hero-sweep-check" },
            Animation::new(Duration::from_millis(dur)).repeat().with_max_fps(60.0),
            move |d, t| {
                d.child(canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    if w <= 2.0 {
                        return;
                    }
                    let band_w = (w * 0.34).max(48.0);
                    let x0 = f32::from(bounds.origin.x);
                    let y0 = f32::from(bounds.origin.y);
                    let h = bounds.size.height;
                    let cx = -band_w * 0.5 + t * (w + band_w);
                    let env = (t * std::f32::consts::PI).sin();
                    const SLICES: usize = 7;
                    let slice_w = band_w / SLICES as f32;
                    for i in 0..SLICES {
                        let u = (i as f32 + 0.5) / SLICES as f32;
                        let bell = (u * std::f32::consts::PI).sin().powi(2);
                        let sx = (cx - band_w * 0.5 + i as f32 * slice_w).max(0.0);
                        let ex = (cx - band_w * 0.5 + (i as f32 + 1.0) * slice_w).min(w);
                        if ex > sx {
                            window.paint_quad(fill(Bounds::new(point(px(x0 + sx), px(y0)), size(px(ex - sx), h)), Hsla { a: peak * env * bell, ..tint }));
                        }
                    }
                }).size_full())
            },
        )
        .into_any_element()
}

/// Animated progress bar for downloading & installing an update:
/// - Indeterminate gliding pill while total bytes are unknown (`total == 0`).
/// - Determinate filled bar with a luminous shimmer sweep + leading-edge glow while downloading.
/// - Brisk full-bar pulse shimmer while verifying & swapping the bundle (`got >= total`).
fn update_progress_bar(c: Colors, got: u64, total: u64) -> AnyElement {
    let phase = install_phase(got, total);
    let share = if total > 0 { (got as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    let accent = c.accent;

    let inner: AnyElement = if phase == InstallPhase::Starting {
        div().size_full().with_animation(
            "update-bar-indeterminate",
            Animation::new(Duration::from_millis(1350)).repeat().with_max_fps(60.0),
            move |d, t| {
                d.child(canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    if w <= 2.0 {
                        return;
                    }
                    let x0 = f32::from(bounds.origin.x);
                    let y0 = f32::from(bounds.origin.y);
                    let e = t * t * (3.0 - 2.0 * t);
                    let pill_w = w * (0.20 + 0.18 * (t * std::f32::consts::PI).sin());
                    let center_x = -pill_w * 0.55 + e * (w + pill_w * 1.1);
                    let left = (center_x - pill_w * 0.5).max(0.0);
                    let right = (center_x + pill_w * 0.5).min(w);
                    if right > left {
                        let radius = Corners::all(px(h * 0.5));
                        window.paint_quad(fill(Bounds::new(point(px(x0 + left), px(y0)), size(px(right - left), px(h))), accent).corner_radii(radius));
                        let core_w = (right - left) * 0.45;
                        let core_l = (left + right - core_w) * 0.5;
                        window.paint_quad(fill(Bounds::new(point(px(x0 + core_l), px(y0)), size(px(core_w), px(h))), hsla(0.0, 0.0, 1.0, 0.26)).corner_radii(radius));
                    }
                }).size_full())
            },
        ).into_any_element()
    } else {
        let finalizing = phase == InstallPhase::Finalizing;
        let dur = if finalizing { 1150 } else { 1450 };
        div().size_full().with_animation(
            if finalizing { "update-bar-finalizing" } else { "update-bar-downloading" },
            Animation::new(Duration::from_millis(dur)).repeat().with_max_fps(60.0),
            move |d, t| {
                d.child(canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    if w <= 2.0 {
                        return;
                    }
                    let x0 = f32::from(bounds.origin.x);
                    let y0 = f32::from(bounds.origin.y);
                    let radius = Corners::all(px(h * 0.5));
                    let fill_w = (w * share.max(0.02)).min(w);
                    window.paint_quad(fill(Bounds::new(point(px(x0), px(y0)), size(px(fill_w), px(h))), accent).corner_radii(radius));

                    // Luminous shimmer sweeping strictly inside the filled bar.
                    let shimmer_w = (w * 0.24).max(36.0).min(fill_w.max(20.0));
                    let cx = -shimmer_w * 0.5 + t * (fill_w + shimmer_w);
                    let peak = if finalizing { 0.42 } else { 0.34 };
                    const SLICES: usize = 6;
                    let slice_w = shimmer_w / SLICES as f32;
                    for i in 0..SLICES {
                        let u = (i as f32 + 0.5) / SLICES as f32;
                        let bell = (u * std::f32::consts::PI).sin().powi(2);
                        let sx = (cx - shimmer_w * 0.5 + i as f32 * slice_w).max(0.0);
                        let ex = (cx - shimmer_w * 0.5 + (i as f32 + 1.0) * slice_w).min(fill_w);
                        if ex > sx {
                            window.paint_quad(fill(Bounds::new(point(px(x0 + sx), px(y0)), size(px(ex - sx), px(h))), hsla(0.0, 0.0, 1.0, peak * bell)));
                        }
                    }

                    // Soft breathing tip highlight at the leading edge while downloading.
                    if !finalizing && fill_w > h {
                        let tip_w = h.min(fill_w);
                        window.paint_quad(
                            fill(Bounds::new(point(px(x0 + fill_w - tip_w), px(y0)), size(px(tip_w), px(h))), hsla(0.0, 0.0, 1.0, 0.22 + 0.18 * breathe(t)))
                                .corner_radii(radius),
                        );
                    }
                }).size_full())
            },
        ).into_any_element()
    };

    div().relative().h(px(6.0)).w_full().rounded_full().bg(c.active).overflow_hidden().child(inner).into_any_element()
}

/// One change of a release: a dot, its area (if it names one) and the text.
fn change_row(c: Colors, change: &str, size: f32) -> Div {
    let (area, text) = updates::split_area(change);
    let mut cap = text.chars();
    let text: String = cap.next().map_or(String::new(), |f| f.to_uppercase().chain(cap).collect());
    div().flex().items_start().gap_2().text_size(px(size)).text_color(c.fg)
        .child(div().mt(px(size * 0.5)).size(px(5.0)).flex_shrink_0().rounded_full().bg(Hsla { a: 0.7, ..c.accent }))
        .child(div().flex_1().min_w_0().flex().flex_wrap().items_center().gap_1p5()
            .when_some(area, |d, a| d.child(div().px_1p5().rounded_sm().bg(c.hover).text_size(px(size - 1.5)).text_color(c.muted).child(a)))
            .child(div().min_w_0().max_w_full().whitespace_normal().child(text)))
}

fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "Stable",
        Channel::Nebula => "Nebula",
    }
}

impl Root {
    /// Background checks: shortly after launch, then every few hours (release builds only), and
    /// the first launch of a new version shows what changed in it.
    pub(super) fn start_updates(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.updates.started, true) {
            return;
        }
        let version = self.updates.version.clone();
        let (saved_channel, last) = {
            let g = self.state.read(cx).rest.get("updates").cloned().unwrap_or_default();
            (g["channel"].as_str().and_then(Channel::parse), g["lastVersion"].as_str().map(str::to_string))
        };
        self.updates.channel = saved_channel.unwrap_or_else(|| Channel::of_version(&version));
        self.set_rest("updates", "lastVersion", json!(version), cx);
        backend::background(updates::cleanup);
        if let Some(last) = last.filter(|l| *l != version) {
            cx.spawn(async move |this, cx| {
                let current = version.clone();
                let notes = backend::background(move || updates::fetch_releases().map(|r| updates::releases_between(&r, Some(&last), &current))).await.ok().and_then(Result::ok).unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    if notes.iter().any(|r| !r.changes.is_empty()) {
                        this.updates.installed = Some((version, notes));
                        this.updates.open = Some(Notes::Installed);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        if !cfg!(debug_assertions) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FIRST_CHECK).await;
                loop {
                    if this.update(cx, |this, cx| this.check_updates(true, cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(CHECK_EVERY).await;
                }
            })
            .detach();
        }
    }

    pub(super) fn set_update_channel(&mut self, channel: Channel, cx: &mut Context<Self>) {
        self.updates.channel = channel;
        self.updates.available = None;
        self.set_rest("updates", "channel", json!(channel.id()), cx);
        self.check_updates(false, cx);
    }

    /// Checks now; `quiet` (background checks) skips the "up to date" and error toasts.
    pub(super) fn check_updates(&mut self, quiet: bool, cx: &mut Context<Self>) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        self.updates.error = None;
        self.updates.status_seq += 1;
        cx.notify();
        let (channel, current) = (self.updates.channel, self.updates.version.clone());
        cx.spawn(async move |this, cx| {
            let found = backend::background(move || {
                let Some(feed) = updates::fetch_feed(channel)? else { return Ok(None) };
                let Some(bundle) = updates::offered(&feed, &current) else { return Ok(None) };
                // The notes come from the releases list; an update without them is still an update.
                let notes = updates::fetch_releases().map(|r| updates::releases_between(&r, Some(&current), &feed.version)).unwrap_or_default();
                Ok::<_, String>(Some(Available { page: updates::release_page(&feed.version), version: feed.version, bundle, notes }))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.updates.channel != channel {
                    return;
                }
                this.updates.checking = false;
                this.updates.status_seq += 1;
                if found.is_ok() {
                    this.updates.checked_at = Some(now_ms());
                }
                match found {
                    Ok(Some(a)) => {
                        let count: usize = a.notes.iter().map(|r| r.changes.len()).sum();
                        let first = this.updates.offered.insert(a.version.clone());
                        let version = a.version.clone();
                        this.updates.available = Some(a);
                        if (first || !quiet) && this.updates.installing.is_none() {
                            let text = format!("winfer {version} is available");
                            if count > 0 {
                                this.toast_action(format!("{text} · {count} change{}", if count == 1 { "" } else { "s" }), "See what's new", Command::ShowReleaseNotes(true), cx);
                            } else {
                                this.toast_action(format!("{text} · restarting stops the agents in your panes"), "Install and restart", Command::InstallUpdate, cx);
                            }
                        }
                    }
                    Ok(None) => {
                        this.updates.available = None;
                        if !quiet {
                            this.toast("winfer is up to date", cx);
                        }
                    }
                    Err(err) => {
                        this.updates.error = Some(err.clone());
                        if !quiet {
                            this.toast_err("Couldn't check for updates", err, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn show_release_notes(&mut self, available: bool, cx: &mut Context<Self>) {
        let which = if available { Notes::Available } else { Notes::Installed };
        if (available && self.updates.available.is_some()) || (!available && self.updates.installed.is_some()) {
            self.updates.open = Some(which);
            cx.notify();
        } else {
            self.toast("No release notes to show", cx);
        }
    }

    /// Opens the update's release page, to install it by hand.
    pub(super) fn download_update(&mut self, cx: &mut Context<Self>) {
        match self.updates.available.clone() {
            Some(a) => {
                cx.open_url(&a.page);
                self.updates.open = None;
                cx.notify();
            }
            None => self.toast("There's no update to download", cx),
        }
    }

    /// Downloads and installs the available update, then quits into it. Nothing installs without
    /// the user's say-so, since restarting stops every running agent.
    pub(super) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.updates.available.clone() else { return self.toast("There's no update to install", cx) };
        if self.updates.installing.is_some() {
            return;
        }
        let progress = Arc::new(Progress::default());
        self.updates.installing = Some(progress.clone());
        self.updates.status_seq += 1;
        self.updates.open = None;
        let admin = if cfg!(windows) { "Windows asks for administrator permission, then " } else { "" };
        self.toast(format!("Downloading winfer {}… {admin}it restarts once it's installed", a.version), cx);
        cx.notify();
        // Redraws the progress until the install ends.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let going = this.update(cx, |this, cx| {
                cx.notify();
                this.updates.installing.is_some()
            });
            if !matches!(going, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let bundle = a.bundle.clone();
            let result = backend::background(move || {
                let exe = updates::install(&bundle, |got, total| {
                    progress.got.store(got, Ordering::Relaxed);
                    progress.total.store(total.unwrap_or(0), Ordering::Relaxed);
                })?;
                updates::relaunch(&exe)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(err) => {
                    this.updates.installing = None;
                    this.updates.status_seq += 1;
                    this.push_toast(Toast::new(ToastKind::Error, format!("Couldn't install winfer {}", a.version)).detail(err.to_string()).action("Download", Command::DownloadUpdate), cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn release_notes_open(&self) -> bool {
        self.updates.open.is_some()
    }

    pub(super) fn close_release_notes(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.updates.open.take().is_some();
        if was {
            cx.notify();
        }
        was
    }

    /// The changes in every release between the current version and the new one: before
    /// downloading an update, and once on the first launch after one.
    pub(super) fn render_whats_new(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = *c;
        let which = self.updates.open?;
        let upcoming = which == Notes::Available;
        let (version, releases) = match (which, &self.updates.available, &self.updates.installed) {
            (Notes::Available, Some(a), _) => (a.version.clone(), a.notes.clone()),
            (Notes::Installed, _, Some((v, r))) => (v.clone(), r.clone()),
            _ => return None,
        };
        let several = releases.len() > 1;
        let shown: Vec<&ReleaseChanges> = releases.iter().filter(|r| !r.changes.is_empty()).collect();
        let subtitle = format!(
            "{}{}",
            if upcoming { format!("You have {}. Restarting stops the agents running in your panes. ", self.updates.version) } else { "Here's what changed since your last version. ".to_string() },
            if several { format!("{} releases.", releases.len()) } else { String::new() }
        );
        let mut body = div().id("whats-new-body").flex().flex_col().gap_4().overflow_y_scroll().px_5().py_4();
        if shown.is_empty() {
            body = body.child(div().text_size(px(13.0)).text_color(c.muted).child("No notes for this release."));
        }
        for r in shown {
            let mut list = div().flex().flex_col().gap_2();
            for change in &r.changes {
                list = list.child(change_row(c, change, 13.0));
            }
            body = body.child(div().flex().flex_col().gap_2().when(several, |d| d.child(div().text_size(px(11.0)).text_color(c.subtle).child(r.version.to_uppercase()))).child(list));
        }
        let installing = self.updates.is_installing();
        let footer = if upcoming {
            let install_btn = if installing {
                div().id("notes-install").flex().items_center().gap_1p5().rounded_md().bg(c.accent).text_color(c.on_accent).px_2p5().py_1().text_size(px(12.0)).opacity(0.75)
                    .child(spinning_icon(IconName::LoaderCircle, 12.0, None, "notes-install-spin"))
                    .child("Installing…")
            } else {
                div().id("notes-install").flex().items_center().gap_1p5().rounded_md().bg(c.accent).text_color(c.on_accent).px_2p5().py_1().text_size(px(12.0)).cursor_pointer().hover(|d| d.opacity(0.9))
                    .on_click(cx.listener(|this, _, _, cx| this.install_update(cx)))
                    .child(Icon::new(IconName::Download).size(px(12.0)))
                    .child("Install and restart")
            };
            div().flex().items_center().justify_end().gap_2()
                .child(div().id("notes-later").rounded_md().px_2p5().py_1().text_size(px(12.0)).text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| { this.close_release_notes(cx); })).child("Later"))
                .child(install_btn)
        } else {
            div().flex().items_center().justify_end()
                .child(div().id("notes-ok").rounded_md().bg(c.accent).text_color(c.on_accent).px_3().py_1().text_size(px(12.0)).cursor_pointer().hover(|d| d.opacity(0.9)).on_click(cx.listener(|this, _, _, cx| { this.close_release_notes(cx); })).child("Got it"))
        };
        Some(
            div().id("whats-new-backdrop").absolute().size_full().flex().items_center().justify_center().bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { this.close_release_notes(cx); }))
                .child(
                    div().id("whats-new").w(px(460.0)).max_h(relative(0.7)).flex().flex_col().rounded_xl().border_1().border_color(c.edge).bg(c.raised).shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().items_start().gap_3().px_5().py_3()
                            .child(div().mt(px(2.0)).child(Icon::new(IconName::Sparkles).size(px(16.0)).text_color(c.accent)))
                            .child(div().flex_1().min_w_0().flex().flex_col()
                                .child(div().text_size(px(14.0)).text_color(c.fg).child(if upcoming { format!("winfer {version} is available") } else { format!("What's new in winfer {version}") }))
                                .child(div().text_size(px(12.0)).text_color(c.subtle).child(subtitle)))
                            .child(div().id("notes-x").rounded_sm().p_0p5().cursor_pointer().hover(move |d| d.bg(c.hover)).on_click(cx.listener(|this, _, _, cx| { this.close_release_notes(cx); })).child(Icon::new(IconName::X).size(px(14.0)).text_color(c.subtle))))
                        .child(div().flex_1().min_h_0().flex().flex_col().child(body))
                        .child(div().px_5().py_3().child(footer)),
                )
                .into_any_element(),
        )
    }

    /// Settings → Updates: the status with its action, the release channel and the release notes.
    pub(super) fn page_updates(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        use super::settings_kit::{alpha, badge, icon_tile, panel, radio, section};
        let u = &self.updates;
        let (channel, checking, version, status_seq) = (u.channel, u.checking, u.version.clone(), u.status_seq);
        let available = u.available.clone();
        let error = u.error.clone().filter(|_| available.is_none());
        let installed = u.installed.clone().filter(|(v, _)| *v == version);
        let progress = u.installing.as_ref().map(|p| (p.got.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed)));
        let checked_at = u.checked_at;
        let mono = crate::fonts::mono(cx);
        let button = |id: &'static str, icon_el: AnyElement, label: String, primary: bool, disabled: bool| {
            div().id(id).h(px(32.0)).flex().items_center().justify_center().gap_1p5().rounded_md().px_3().text_size(px(12.5)).flex_shrink_0()
                .when(!disabled, |d| d.cursor_pointer())
                .when(disabled, |d| d.opacity(0.65))
                .when(primary, |d| d.bg(c.accent).text_color(c.on_accent).font_weight(FontWeight::MEDIUM).when(!disabled, |d| d.hover(|d| d.opacity(0.9))))
                .when(!primary, |d| d.border_1().border_color(c.edge).bg(c.raised).text_color(c.fg).when(!disabled, |d| d.hover(move |d| d.bg(c.hover))))
                .child(icon_el)
                .child(label)
        };
        let active = checking || progress.is_some();
        let (icon, tint) = match (&available, &error, progress, checking) {
            (Some(_), _, Some(_), _) => (IconName::LoaderCircle, c.accent),
            (_, _, None, true) => (IconName::RefreshCw, c.accent),
            (Some(_), _, None, false) => (IconName::Download, c.accent),
            (None, Some(_), _, false) => (IconName::CircleAlert, c.danger),
            _ => (IconName::CircleCheck, c.success),
        };
        let title = match (&available, &error, progress, checking) {
            (Some(a), _, Some((got, total)), _) => match install_phase(got, total) {
                InstallPhase::Finalizing => format!("Installing winfer {}…", a.version),
                InstallPhase::Starting | InstallPhase::Downloading => format!("Downloading winfer {}…", a.version),
            },
            (_, _, None, true) => "Checking for updates…".to_string(),
            (Some(a), _, None, false) => format!("winfer {} is available", a.version),
            (None, Some(_), _, false) => "Couldn't check for updates".to_string(),
            _ => "winfer is up to date".to_string(),
        };
        let changes = |notes: &[ReleaseChanges]| notes.iter().map(|r| r.changes.len()).sum::<usize>();
        let subtitle = match (&available, &error, progress, checking) {
            (Some(_), _, Some((got, total)), _) => match install_phase(got, total) {
                InstallPhase::Finalizing => "Verifying signature and replacing winfer — restarting shortly".to_string(),
                InstallPhase::Starting | InstallPhase::Downloading => "winfer restarts once it's installed".to_string(),
            },
            (_, _, None, true) => format!("Looking for the latest {} release", channel_name(channel)),
            (Some(a), _, None, false) => {
                let n = changes(&a.notes);
                let releases = a.notes.len();
                format!("You have {version}{}{}", if n > 0 { format!(" · {n} change{}", if n == 1 { "" } else { "s" }) } else { String::new() }, if releases > 1 { format!(", {releases} releases") } else { String::new() })
            }
            (None, Some(e), _, false) => e.clone(),
            _ if channel == Channel::Stable && version.contains('-') => format!("{version} · moves to Stable with the next stable release"),
            _ => format!("You have {version}, the latest {} release", channel_name(channel)),
        };
        let action = match (&available, progress) {
            (Some(_), Some(_)) => None,
            (Some(_), None) => Some(button("update-install", Icon::new(IconName::Download).size(px(13.0)).into_any_element(), "Install and restart".into(), true, false).on_click(cx.listener(|this, _, _, cx| this.install_update(cx))).into_any_element()),
            (None, _) => {
                let icon_el: AnyElement = if checking {
                    spinning_icon(IconName::RefreshCw, 13.0, None, "update-check-btn-spin").into_any_element()
                } else {
                    Icon::new(IconName::RefreshCw).size(px(13.0)).into_any_element()
                };
                Some(button("update-check", icon_el, if checking { "Checking…".into() } else { "Check now".into() }, false, checking).on_click(cx.listener(|this, _, _, cx| this.check_updates(false, cx))).into_any_element())
            }
        };
        let top = div().flex().items_center().gap_3p5()
            .child(update_hero_tile(tint, checking, progress, icon, status_seq))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(title))
                .child(div().truncate().text_size(px(12.0)).text_color(if error.is_some() && !checking { c.danger } else { c.muted }).child(subtitle)))
            .children(action);
        let bar = progress.map(|(got, total)| {
            let phase = install_phase(got, total);
            let mb = |b: u64| format!("{:.1} MB", b as f64 / 1_048_576.0);
            let status_text = match phase {
                InstallPhase::Starting => "Starting the download…".to_string(),
                InstallPhase::Downloading => format!("{} of {} · {}%", mb(got), mb(total), (got.min(total) * 100) / total),
                InstallPhase::Finalizing => "Verifying & installing…".to_string(),
            };
            let status_el: AnyElement = if phase != InstallPhase::Downloading {
                div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(status_text)
                    .with_animation("update-bar-status-pulse", Animation::new(Duration::from_millis(1400)).repeat().with_max_fps(30.0), |d, t| d.opacity(0.6 + 0.4 * breathe(t)))
                    .into_any_element()
            } else {
                div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(status_text).into_any_element()
            };
            div().flex().items_center().gap_3()
                .child(div().flex_1().child(update_progress_bar(c, got, total)))
                .child(status_el)
        });
        let fact = |icon: IconName, text: String, color: Hsla| div().flex().items_center().gap_1p5().text_color(color).child(Icon::new(icon).size(px(12.0))).child(text);
        let checked_fact: AnyElement = match (checking, checked_at) {
            (true, _) => div().flex().items_center().gap_1p5().text_color(c.accent)
                .child(Icon::new(IconName::Clock).size(px(12.0)))
                .child("Checking…")
                .with_animation("update-meta-checking", Animation::new(Duration::from_millis(1400)).repeat().with_max_fps(30.0), |d, t| d.opacity(0.55 + 0.45 * breathe(t)))
                .into_any_element(),
            (false, Some(at)) => {
                let label = match super::dialogs::relative_time(at).as_str() {
                    "now" => "Checked just now".to_string(),
                    ago => format!("Checked {ago} ago"),
                };
                fact(IconName::Clock, label, c.subtle).into_any_element()
            }
            (false, None) => fact(IconName::Clock, "Not checked since winfer started".to_string(), c.subtle).into_any_element(),
        };
        let meta = div().flex().flex_wrap().items_center().gap_x_4().gap_y_1().pt_3().border_t_1().border_color(c.border).text_size(px(11.5))
            .child(checked_fact)
            .when(!cfg!(debug_assertions), |d| d.child(fact(IconName::RefreshCcw, "winfer checks every 6 hours".into(), c.subtle)))
            .when(available.is_some(), |d| d.child(fact(IconName::TriangleAlert, "Restarting stops the agents in your panes".into(), c.warning)));
        let hero = panel(c).relative().overflow_hidden().gap_3().p_4()
            .when(available.is_some(), |d| d.border_color(alpha(c.accent, 0.6)).bg(alpha(c.accent, 0.05)))
            .when(checking && available.is_none(), |d| d.border_color(alpha(c.accent, 0.4)).bg(alpha(c.accent, 0.03)))
            .when(active, |d| d.child(update_hero_sweep(c.accent, progress.is_some())))
            .child(top)
            .children(bar)
            .child(meta);

        let choice = |id: &'static str, target: Channel, icon: IconName, tint: Hsla, title: &'static str, preview: bool, text: &'static str| {
            let on = channel == target;
            div().id(id).flex_1().min_w_0().flex().items_start().gap_3().p_3().rounded(px(10.0)).border_1().cursor_pointer()
                .border_color(if on { c.accent } else { c.edge }).bg(c.panel)
                .when(on, |d| d.border_2())
                .when(!on, |d| d.hover(move |d| d.bg(c.hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.updates.channel != target {
                        this.set_update_channel(target, cx);
                    }
                }))
                .child(icon_tile(tint, icon, 30.0))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().flex().items_center().gap_2().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title).when(preview, |d| d.child(badge(c.accent, "preview"))))
                    .child(div().text_size(px(12.0)).text_color(c.subtle).child(text)))
                .child(radio(c, on))
        };
        let channels = div().flex().gap_3()
            .child(choice("channel-stable", Channel::Stable, IconName::ShieldCheck, c.success, "Stable", false, "Tested releases."))
            .child(choice("channel-nebula", Channel::Nebula, IconName::Sparkles, c.accent, "Nebula", true, "Every change as soon as it lands. Newest features, rougher edges."));
        let leaving_nebula = (channel == Channel::Stable && version.contains('-')).then(|| {
            div().flex().items_start().gap_2p5().px_3().py_2p5().rounded(px(9.0)).border_1().border_color(c.edge).bg(c.panel).text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::Info).size(px(14.0)).text_color(c.accent).flex_shrink_0())
                .child(div().flex_1().min_w_0().child(format!("This is a Nebula build ({version}). Stable releases are older, so winfer stays on it until the next stable release, then moves over on its own.")))
        });

        let (notes_title, notes_note, notes) = match (&available, &installed) {
            (Some(a), _) if changes(&a.notes) > 0 => (format!("What's coming in {}", a.version), (a.notes.len() > 1).then(|| format!("{} releases since yours", a.notes.len())), a.notes.clone()),
            (_, Some((v, r))) if changes(r) > 0 => (format!("What's new in {v}"), (r.len() > 1).then(|| format!("{} releases since your last version", r.len())), r.clone()),
            _ => (String::new(), None, Vec::new()),
        };
        let notes_list = (!notes.is_empty()).then(|| {
            let mut list = panel(c).overflow_hidden();
            for (i, r) in notes.iter().filter(|r| !r.changes.is_empty()).enumerate() {
                let mut block = div().flex().flex_col().gap_1p5().px_4().py_3().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(div().font_family(mono.clone()).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(r.version.clone()));
                for change in &r.changes {
                    block = block.child(change_row(c, change, 12.5));
                }
                list = list.child(block);
            }
            list
        });

        div().flex().flex_col().gap_4()
            .child(hero)
            .child(section(c, "Release channel", None))
            .child(channels)
            .children(leaving_nebula)
            .when_some(notes_list, |d, list| d.child(section(c, &notes_title, notes_note)).child(list))
    }
}

#[cfg(test)]
mod tests {
    use super::{InstallPhase, Progress, UpdatesUi, breathe, ease_out, install_phase};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    #[test]
    fn install_phase_distinguishes_starting_downloading_and_finalizing() {
        assert_eq!(install_phase(0, 0), InstallPhase::Starting);
        assert_eq!(install_phase(1024, 0), InstallPhase::Starting);
        assert_eq!(install_phase(0, 100), InstallPhase::Downloading);
        assert_eq!(install_phase(50, 100), InstallPhase::Downloading);
        assert_eq!(install_phase(100, 100), InstallPhase::Finalizing);
        assert_eq!(install_phase(120, 100), InstallPhase::Finalizing);
    }

    #[test]
    fn install_percent_tracks_and_clamps_progress() {
        let mut ui = UpdatesUi::default();
        assert!(!ui.is_installing());
        assert_eq!(ui.install_percent(), None);

        let p = Arc::new(Progress::default());
        ui.installing = Some(p.clone());
        assert!(ui.is_installing());
        assert_eq!(ui.install_percent(), None);

        p.total.store(200, Ordering::Relaxed);
        p.got.store(50, Ordering::Relaxed);
        assert_eq!(ui.install_percent(), Some(25));

        p.got.store(250, Ordering::Relaxed);
        assert_eq!(ui.install_percent(), Some(100));
    }

    #[test]
    fn animation_curves_stay_within_unit_interval() {
        assert!((ease_out(0.0) - 0.0).abs() < 1e-6);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-6);
        assert!(ease_out(0.5) > 0.5);
        for step in 0..=16 {
            let t = step as f32 / 16.0;
            let b = breathe(t);
            assert!((0.0..=1.0).contains(&b), "breathe({t}) = {b} out of bounds");
        }
    }
}
