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

impl Progress {
    /// "42%", or "" while the size isn't known.
    fn percent(&self) -> String {
        let (got, total) = (self.got.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        if total == 0 { String::new() } else { format!(" {}%", got * 100 / total) }
    }
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
}

impl Default for UpdatesUi {
    fn default() -> Self {
        Self { version: env!("CARGO_PKG_VERSION").to_string(), channel: Channel::Stable, checking: false, error: None, available: None, installed: None, open: None, offered: HashSet::new(), started: false, installing: None }
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

    /// "Installing… 42%" while an update installs.
    fn install_label(&self) -> Option<String> {
        self.updates.installing.as_ref().map(|p| format!("Installing…{}", p.percent()))
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
                let (area, text) = updates::split_area(change);
                let mut cap = text.chars();
                let text: String = cap.next().map_or(String::new(), |f| f.to_uppercase().chain(cap).collect());
                list = list.child(div().flex().items_start().gap_2().text_size(px(13.0)).text_color(c.fg)
                    .child(div().mt(px(7.0)).size(px(6.0)).flex_shrink_0().rounded_full().bg(Hsla { a: 0.7, ..c.accent }))
                    .child(div().flex_1().min_w_0().flex().flex_wrap().items_center().gap_1p5()
                        .when_some(area, |d, a| d.child(div().px_1p5().rounded_sm().bg(c.hover).text_size(px(11.0)).text_color(c.muted).child(a)))
                        .child(div().min_w_0().max_w_full().whitespace_normal().child(text))));
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

    /// App version, channel and update actions, pinned to the bottom of the settings sidebar.
    /// Settings → Updates: the release channel, the available update and what's new.
    pub(super) fn page_updates(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let u = &self.updates;
        let (channel, checking, version) = (u.channel, u.checking, u.version.clone());
        let available = u.available.clone();
        let error = u.error.clone();
        let has_installed = u.installed.as_ref().is_some_and(|(v, _)| *v == version);
        let installing = self.install_label();
        let mono = crate::fonts::mono(cx);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div().id(id).h(px(30.0)).flex().items_center().justify_center().gap_1p5().rounded_md().px_3().text_size(px(12.5)).cursor_pointer().flex_shrink_0()
                .when(primary, |d| d.bg(c.accent).text_color(c.on_accent).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(c.edge).text_color(c.fg).hover(move |d| d.bg(c.hover)))
                .child(label)
        };
        let hero = div().flex().items_center().gap_3p5().p_4().rounded(px(10.0)).border_1()
            .bg(if available.is_some() { Hsla { a: 0.08, ..c.accent } } else { c.panel })
            .child(div().size(px(40.0)).flex_shrink_0().flex().items_center().justify_center().rounded_lg()
                .bg(if available.is_some() { c.accent } else { c.hover }).text_color(if available.is_some() { c.on_accent } else { c.muted })
                .child(Icon::new(if available.is_some() { IconName::Download } else { IconName::CircleCheck }).size(px(19.0))))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(match &available {
                    Some(a) if installing.is_some() => format!("Installing winfer {}", a.version),
                    Some(a) => format!("winfer {} is available", a.version),
                    None => "winfer is up to date".to_string(),
                }))
                .child(div().text_size(px(12.0)).text_color(c.muted).font_family(mono).child(match (&available, &error) {
                    (_, Some(e)) => e.clone(),
                    (Some(_), _) if installing.is_some() => "winfer restarts once it's installed".to_string(),
                    (Some(_), _) => format!("You have {version} · restarting stops the agents in your panes"),
                    (None, _) if channel == Channel::Stable && version.contains('-') => format!("{version} · moves to stable with the next stable release"),
                    (None, _) => version.clone(),
                })))
            .when(available.is_some() || has_installed, |d| d.child(button("update-notes", "What's new", false).on_click(cx.listener(move |this, _, _, cx| this.show_release_notes(this.updates.available.is_some(), cx)))))
            .child(match (available, installing) {
                (Some(_), Some(label)) => button("update-install", "", true).opacity(0.7).cursor_default().child(label),
                (Some(_), None) => button("update-install", "Install and restart", true).on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                (None, _) => button("update-check", if checking { "Checking…" } else { "Check now" }, false).on_click(cx.listener(|this, _, _, cx| this.check_updates(false, cx))),
            });
        let seg = |id: &'static str, label: &'static str, target: Channel| {
            let on = channel == target;
            div().id(id).h(px(24.0)).px_2p5().flex().items_center().rounded(px(6.0)).text_size(px(12.0)).cursor_pointer()
                .text_color(if on { c.fg } else { c.muted }).when(on, |d| d.bg(c.active))
                .hover(move |d| d.text_color(c.fg))
                .on_click(cx.listener(move |this, _, _, cx| this.set_update_channel(target, cx)))
                .child(label)
        };
        let channel_row = super::settings::row(c, "Channel", Some("Stable: tested releases. Nebula: every change as soon as it lands.".into()),
            div().flex().gap_0p5().p(px(2.0)).rounded(px(8.0)).border_1().border_color(c.edge).bg(c.app)
                .child(seg("channel-stable", "Stable", Channel::Stable)).child(seg("channel-nebula", "Nebula", Channel::Nebula)));
        div().flex().flex_col().gap_5().child(hero).child(super::settings::card(c, "Release channel", None).child(channel_row))
    }
}
