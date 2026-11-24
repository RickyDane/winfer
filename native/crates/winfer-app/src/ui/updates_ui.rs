//! Update checks and installs, the "What's new" dialog and Settings → Updates (port of
//! `features/updates/updater.ts`, `WhatsNew.tsx` and the update footer of `SettingsModal.tsx`).

use super::commands::Command;
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
}

impl Default for UpdatesUi {
    fn default() -> Self {
        Self { version: env!("CARGO_PKG_VERSION").to_string(), channel: Channel::Stable, checking: false, error: None, available: None, installed: None, open: None, offered: HashSet::new(), started: false, installing: None, checked_at: None }
    }
}

impl UpdatesUi {
    /// Development aid: the dialog with made-up notes.
    #[cfg(debug_assertions)]
    pub(super) fn preview(&mut self) {
        let release = |v: &str, c: &[&str]| ReleaseChanges { version: v.into(), changes: c.iter().map(|s| s.to_string()).collect() };
        self.installed = Some((self.version.clone(), vec![release("0.2.0", &["Browser: phone view and page handles", "chat pane for winfer code", "fix crash on start"]), release("0.1.9", &["Viewer: PDFs"])]));
        self.open = Some(Notes::Installed);
    }
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
                            this.toast(format!("Couldn't check for updates: {err}"), cx);
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
        self.updates.open = None;
        self.toast(format!("Downloading winfer {}… it restarts once it's installed", a.version), cx);
        cx.notify();
        // Redraws the progress until the install ends.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
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
                    this.toast_with_label(format!("Couldn't install winfer {}: {err}", a.version), "Download".into(), Command::DownloadUpdate, cx);
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
        let footer = if upcoming {
            div().flex().items_center().justify_end().gap_2()
                .child(div().id("notes-later").rounded_md().px_2p5().py_1().text_size(px(12.0)).text_color(c.muted).cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)).on_click(cx.listener(|this, _, _, cx| { this.close_release_notes(cx); })).child("Later"))
                .child(div().id("notes-install").flex().items_center().gap_1p5().rounded_md().bg(c.accent).text_color(c.on_accent).px_2p5().py_1().text_size(px(12.0)).cursor_pointer().hover(|d| d.opacity(0.9)).on_click(cx.listener(|this, _, _, cx| this.install_update(cx))).child(Icon::new(IconName::Download).size(px(12.0))).child("Install and restart"))
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
        use super::settings_kit::{alpha, badge, icon_tile, meter, panel, radio, section};
        let u = &self.updates;
        let (channel, checking, version) = (u.channel, u.checking, u.version.clone());
        let available = u.available.clone();
        let error = u.error.clone().filter(|_| available.is_none());
        let installed = u.installed.clone().filter(|(v, _)| *v == version);
        let progress = u.installing.as_ref().map(|p| (p.got.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed)));
        let checked_at = u.checked_at;
        let mono = crate::fonts::mono(cx);
        let button = |id: &'static str, icon: IconName, label: String, primary: bool| {
            div().id(id).h(px(32.0)).flex().items_center().justify_center().gap_1p5().rounded_md().px_3().text_size(px(12.5)).cursor_pointer().flex_shrink_0()
                .when(primary, |d| d.bg(c.accent).text_color(c.on_accent).font_weight(FontWeight::MEDIUM).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(c.edge).bg(c.raised).text_color(c.fg).hover(move |d| d.bg(c.hover)))
                .child(Icon::new(icon).size(px(13.0)))
                .child(label)
        };
        let (icon, tint) = match (&available, &error, progress) {
            (Some(_), _, Some(_)) => (IconName::LoaderCircle, c.accent),
            (Some(_), _, None) => (IconName::Download, c.accent),
            (None, Some(_), _) => (IconName::CircleAlert, c.danger),
            _ => (IconName::CircleCheck, c.success),
        };
        let title = match (&available, &error, progress) {
            (Some(a), _, Some(_)) => format!("Installing winfer {}", a.version),
            (Some(a), _, None) => format!("winfer {} is available", a.version),
            (None, Some(_), _) => "Couldn't check for updates".to_string(),
            _ => "winfer is up to date".to_string(),
        };
        let changes = |notes: &[ReleaseChanges]| notes.iter().map(|r| r.changes.len()).sum::<usize>();
        let subtitle = match (&available, &error, progress) {
            (Some(_), _, Some(_)) => "winfer restarts once it's installed".to_string(),
            (Some(a), _, None) => {
                let n = changes(&a.notes);
                let releases = a.notes.len();
                format!("You have {version}{}{}", if n > 0 { format!(" · {n} change{}", if n == 1 { "" } else { "s" }) } else { String::new() }, if releases > 1 { format!(", {releases} releases") } else { String::new() })
            }
            (None, Some(e), _) => e.clone(),
            _ if channel == Channel::Stable && version.contains('-') => format!("{version} · moves to Stable with the next stable release"),
            _ => format!("You have {version}, the latest {} release", channel_name(channel)),
        };
        let action = match (&available, progress) {
            (Some(_), Some(_)) => None,
            (Some(_), None) => Some(button("update-install", IconName::Download, "Install and restart".into(), true).on_click(cx.listener(|this, _, _, cx| this.install_update(cx))).into_any_element()),
            (None, _) => Some(button("update-check", IconName::RefreshCw, if checking { "Checking…".into() } else { "Check now".into() }, false).when(checking, |d| d.opacity(0.6)).on_click(cx.listener(|this, _, _, cx| this.check_updates(false, cx))).into_any_element()),
        };
        let top = div().flex().items_center().gap_3p5()
            .child(icon_tile(tint, icon, 40.0))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(title))
                .child(div().truncate().text_size(px(12.0)).text_color(if error.is_some() { c.danger } else { c.muted }).child(subtitle)))
            .children(action);
        let bar = progress.map(|(got, total)| {
            let share = if total > 0 { got as f32 / total as f32 } else { 0.0 };
            let mb = |b: u64| format!("{:.0} MB", b as f64 / 1_048_576.0);
            div().flex().items_center().gap_3()
                .child(div().flex_1().child(meter(c, share, c.accent, 6.0)))
                .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(if total > 0 { format!("{} of {} · {}%", mb(got), mb(total), got * 100 / total) } else { "Starting the download…".into() }))
        });
        let fact = |icon: IconName, text: String, color: Hsla| div().flex().items_center().gap_1p5().text_color(color).child(Icon::new(icon).size(px(12.0))).child(text);
        let checked = match (checking, checked_at) {
            (true, _) => "Checking…".to_string(),
            (false, Some(at)) => match super::dialogs::relative_time(at).as_str() {
                "now" => "Checked just now".to_string(),
                ago => format!("Checked {ago} ago"),
            },
            (false, None) => "Not checked since winfer started".to_string(),
        };
        let meta = div().flex().flex_wrap().items_center().gap_x_4().gap_y_1().pt_3().border_t_1().border_color(c.border).text_size(px(11.5))
            .child(fact(IconName::Clock, checked, c.subtle))
            .when(!cfg!(debug_assertions), |d| d.child(fact(IconName::RefreshCcw, "winfer checks every 6 hours".into(), c.subtle)))
            .when(available.is_some(), |d| d.child(fact(IconName::TriangleAlert, "Restarting stops the agents in your panes".into(), c.warning)));
        let hero = panel(c).gap_3().p_4()
            .when(available.is_some(), |d| d.border_color(alpha(c.accent, 0.6)).bg(alpha(c.accent, 0.05)))
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
