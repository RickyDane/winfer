//! In-panel file browser for paired remote machines (`file.call` `"list"` / `"stat"`).
//! Lives inside `ViewerPanel` so users can pick a machine, jump to any of its workspaces or `~`,
//! filter or type a path, and open any file directly in the right side panel.

use super::{IMAGE, ViewerPanel, extension, video};
use crate::theme::Colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;
use winfer_connect::pairing::Kind;
use winfer_core::connect_files;

#[derive(Clone, Debug)]
pub(super) struct RemoteEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub git: bool,
}

pub(super) struct RemoteFileBrowser {
    pub peer: String,
    pub input: Entity<InputState>,
    /// Display path (`~/project`, `/etc`).
    pub display_path: String,
    /// Canonical absolute path on the remote machine.
    pub abs_path: String,
    /// Parent directory's absolute path.
    pub parent: Option<String>,
    pub entries: Option<Vec<RemoteEntry>>,
    pub truncated: bool,
    pub loading: bool,
    pub error: Option<String>,
    seq: u64,
}

/// Online paired machines: `(peer_id, name, is_server)`.
pub(super) fn online_machines() -> Vec<(String, String, bool)> {
    crate::connect::get()
        .map(|c| {
            c.status()
                .peers
                .into_iter()
                .filter(|p| p.link.via.is_some())
                .map(|p| (p.peer_id, p.name, p.kind == Kind::Server))
                .collect()
        })
        .unwrap_or_default()
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn file_icon(name: &str) -> IconName {
    let ext = extension(name);
    if IMAGE.contains(&ext.as_str()) {
        IconName::Image
    } else if video::is_video(name) {
        IconName::Video
    } else if matches!(ext.as_str(), "md" | "markdown" | "mdx" | "txt" | "pdf") {
        IconName::FileText
    } else {
        IconName::FileCode
    }
}

fn looks_like_path(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('/')
        || t.starts_with('~')
        || t.starts_with('\\')
        || (t.len() >= 3 && t.as_bytes()[1] == b':' && matches!(t.as_bytes()[2], b'/' | b'\\'))
}

impl ViewerPanel {
    /// Opens the in-panel remote file browser for `peer` (default: active remote doc's peer, else
    /// first online paired machine) at `start_path` (default: its first workspace root, else `~`).
    pub fn browse_remote(&mut self, peer: Option<String>, start_path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let target = peer
            .or_else(|| self.shown.as_deref().and_then(|d| connect_files::split_doc(d).0).map(str::to_string))
            .or_else(|| online_machines().into_iter().next().map(|(id, _, _)| id));
        let Some(peer) = target else { return };
        let initial = start_path
            .filter(|p| !p.trim().is_empty())
            .or_else(|| connect_files::workspaces_of(&peer).into_iter().find(|w| !w.root_dir.is_empty()).map(|w| w.root_dir))
            .unwrap_or_else(|| "~".to_string());

        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files, or enter a path…"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value().trim().to_string();
                this.remote_browser_enter(&text, window, cx);
            }
            _ => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.remote_browser = Some(RemoteFileBrowser {
            peer,
            input,
            display_path: initial.clone(),
            abs_path: initial.clone(),
            parent: None,
            entries: None,
            truncated: false,
            loading: true,
            error: None,
            seq: 0,
        });
        self.remote_list(initial, cx);
    }

    pub fn close_remote_browser(&mut self, cx: &mut Context<Self>) {
        if self.remote_browser.take().is_some() {
            cx.notify();
        }
    }

    fn remote_switch_machine(&mut self, peer: String, window: &mut Window, cx: &mut Context<Self>) {
        let initial = connect_files::workspaces_of(&peer)
            .into_iter()
            .find(|w| !w.root_dir.is_empty())
            .map(|w| w.root_dir)
            .unwrap_or_else(|| "~".to_string());
        if let Some(b) = self.remote_browser.as_mut() {
            if b.peer == peer {
                return;
            }
            b.peer = peer;
            b.entries = None;
            b.parent = None;
            b.error = None;
            b.display_path = initial.clone();
            b.abs_path = initial.clone();
            let input = b.input.clone();
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.remote_list(initial, cx);
    }

    fn remote_go(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.remote_browser.as_ref() {
            b.input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.remote_list(path, cx);
    }

    fn remote_list(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(b) = self.remote_browser.as_mut() else { return };
        b.seq += 1;
        let (peer, seq) = (b.peer.clone(), b.seq);
        b.loading = true;
        b.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = connect_files::call(&peer, "list", json!({ "path": path })).await;
            let _ = this.update(cx, |this, cx| {
                let Some(b) = this.remote_browser.as_mut().filter(|b| b.peer == peer && b.seq == seq) else { return };
                b.loading = false;
                match res {
                    Ok(v) => {
                        b.display_path = v["path"].as_str().unwrap_or(&path).to_string();
                        b.abs_path = v["absPath"].as_str().unwrap_or(&b.display_path).to_string();
                        b.parent = v["parent"].as_str().map(str::to_string);
                        b.truncated = v["truncated"] == true;
                        b.entries = Some(
                            v["entries"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|e| {
                                    Some(RemoteEntry {
                                        name: e["name"].as_str()?.to_string(),
                                        dir: e["dir"] == true,
                                        size: e["size"].as_u64().unwrap_or(0),
                                        git: e["git"] == true,
                                    })
                                })
                                .collect(),
                        );
                    }
                    Err(e) => b.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remote_browser_enter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.remote_browser.as_mut() else { return };
        if text.is_empty() {
            return;
        }
        if looks_like_path(text) {
            b.seq += 1;
            let (peer, seq, query) = (b.peer.clone(), b.seq, text.to_string());
            b.loading = true;
            b.error = None;
            cx.notify();
            let input = b.input.clone();
            cx.spawn_in(window, async move |this, cx| {
                let st = connect_files::call(&peer, "stat", json!({ "path": query })).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let Some(b) = this.remote_browser.as_mut().filter(|b| b.peer == peer && b.seq == seq) else { return };
                    b.loading = false;
                    match st {
                        Ok(v) if v["exists"] == true && v["isFile"] == true => {
                            let abs = v["absPath"].as_str().unwrap_or(&query).to_string();
                            this.open_remote(&peer, abs, None, false, cx);
                        }
                        Ok(v) if v["exists"] == true && v["isDir"] == true => {
                            let abs = v["absPath"].as_str().unwrap_or(&query).to_string();
                            input.update(cx, |i, cx| i.set_value("", window, cx));
                            this.remote_list(abs, cx);
                        }
                        Ok(_) => {
                            b.error = Some(format!("No file or folder at {query}"));
                            cx.notify();
                        }
                        Err(e) => {
                            b.error = Some(e);
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
            return;
        }
        let q = text.to_lowercase();
        let matches: Vec<RemoteEntry> = b
            .entries
            .iter()
            .flatten()
            .filter(|e| e.name.to_lowercase().contains(&q))
            .cloned()
            .collect();
        if let [only] = &matches[..] {
            let full = connect_files::join_remote_path(&b.abs_path, &only.name);
            let peer = b.peer.clone();
            if only.dir {
                self.remote_go(full, window, cx);
            } else {
                self.open_remote(&peer, full, None, false, cx);
            }
        }
    }

    pub(super) fn render_remote_browser(&mut self, c: Colors, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(b) = self.remote_browser.as_ref() else {
            return div().into_any_element();
        };
        let peer = b.peer.clone();
        let peer_name = crate::connect::peer_name(&peer).unwrap_or_else(|| "Remote machine".into());
        let machines = online_machines();
        let workspaces = connect_files::workspaces_of(&peer);
        let query = b.input.read(cx).value().trim().to_string();
        let q = query.to_lowercase();
        let display_path = b.display_path.clone();
        let abs_path = b.abs_path.clone();
        let parent = b.parent.clone();
        let entries = b.entries.clone();
        let truncated = b.truncated;
        let loading = b.loading;
        let error = b.error.clone();
        let input = b.input.clone();
        let has_open_doc = self.shown.is_some();

        // Header bar: title + close button.
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_2p5()
            .py_1p5()
            .border_b_1()
            .border_color(c.border)
            .child(Icon::new(IconName::Server).size(px(13.0)).text_color(c.accent))
            .child(div().flex_1().min_w_0().truncate().text_xs().font_weight(FontWeight::MEDIUM).text_color(c.fg).child(format!("Browse files on {peer_name}")))
            .when(has_open_doc, |d| {
                d.child(
                    div()
                        .id("rb-close")
                        .rounded_sm()
                        .p_1()
                        .cursor_pointer()
                        .text_color(c.subtle)
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .tooltip(|window, cx| Tooltip::new("Back to file").build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.close_remote_browser(cx)))
                        .child(Icon::new(IconName::X).size(px(13.0))),
                )
            });

        // Machine switcher chips (like AndroidPanel).
        let mut listed = machines.clone();
        if !listed.iter().any(|(id, _, _)| id == &peer) {
            listed.push((peer.clone(), format!("{peer_name} (offline)"), true));
        }
        let switcher = (listed.len() > 1).then(|| {
            let mut row = div().flex().flex_wrap().items_center().gap_1().px_2p5().pt_1p5().text_xs();
            for (i, (id, name, is_server)) in listed.into_iter().enumerate() {
                let on = id == peer;
                row = row.child(
                    div()
                        .id(("rb-machine", i))
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded_md()
                        .border_1()
                        .px_1p5()
                        .py_0p5()
                        .cursor_pointer()
                        .when(on, |d| d.border_color(c.accent).text_color(c.fg).bg(Hsla { a: 0.1, ..c.accent }))
                        .when(!on, |d| d.border_color(c.edge).text_color(c.muted).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, window, cx| this.remote_switch_machine(id.clone(), window, cx)))
                        .child(Icon::new(if is_server { IconName::Server } else { IconName::Monitor }).size(px(11.0)))
                        .child(name),
                );
            }
            row
        });

        // Quick-jump chips for this machine's workspaces + home (`~`).
        let ws_chips = (!workspaces.is_empty()).then(|| {
            let norm_cur = super::normalized(&abs_path);
            let mut row = div().flex().flex_wrap().items_center().gap_1().px_2p5().pt_1p5().text_size(px(11.0));
            row = row.child(
                div()
                    .id("rb-jump-home")
                    .rounded_md()
                    .px_1p5()
                    .py_0p5()
                    .cursor_pointer()
                    .text_color(c.muted)
                    .bg(c.raised)
                    .border_1()
                    .border_color(c.edge)
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .on_click(cx.listener(|this, _, window, cx| this.remote_go("~".into(), window, cx)))
                    .child("~"),
            );
            for (i, ws) in workspaces.iter().enumerate() {
                if ws.root_dir.is_empty() {
                    continue;
                }
                let root = ws.root_dir.clone();
                let norm_root = super::normalized(&root);
                let active = !norm_root.is_empty() && (norm_cur == norm_root || norm_cur.starts_with(&format!("{norm_root}/")));
                row = row.child(
                    div()
                        .id(("rb-jump-ws", i))
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded_md()
                        .px_1p5()
                        .py_0p5()
                        .cursor_pointer()
                        .border_1()
                        .when(active, |d| d.border_color(c.accent).text_color(c.fg).bg(Hsla { a: 0.1, ..c.accent }))
                        .when(!active, |d| d.border_color(c.edge).text_color(c.muted).bg(c.raised).hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .on_click(cx.listener(move |this, _, window, cx| this.remote_go(root.clone(), window, cx)))
                        .child(Icon::new(IconName::Folder).size(px(10.0)).text_color(if active { c.accent } else { c.subtle }))
                        .child(ws.name.clone()),
                );
            }
            row
        });

        // Search / path input.
        let search = div()
            .mx_2p5()
            .mt_2()
            .mb_1()
            .h(px(32.0))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .bg(if c.light { hsla(0.0, 0.0, 0.0, 0.05) } else { hsla(0.0, 0.0, 0.0, 0.25) })
            .border_1()
            .border_color(c.edge)
            .child(Icon::new(IconName::Search).size(px(13.0)).text_color(c.subtle))
            .child(div().flex_1().min_w_0().child(Input::new(&input).appearance(false)));

        // Current path + refresh bar.
        let reload_path = abs_path.clone();
        let path_bar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2p5()
            .py_1()
            .border_b_1()
            .border_color(c.border)
            .child(div().flex_1().min_w_0().truncate().text_size(px(11.0)).text_color(c.muted).child(display_path))
            .child(
                div()
                    .id("rb-refresh")
                    .rounded_sm()
                    .p_1()
                    .cursor_pointer()
                    .text_color(c.subtle)
                    .hover(move |d| d.bg(c.hover).text_color(c.fg))
                    .tooltip(|window, cx| Tooltip::new("Refresh folder").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.remote_list(reload_path.clone(), cx)))
                    .child(Icon::new(IconName::RotateCw).size(px(12.0))),
            );

        // Entries list.
        let mut list = div().id("rb-list").flex_1().min_h_0().overflow_y_scroll().p_1p5().flex().flex_col().gap_0p5();
        if let Some(err) = error {
            let retry_path = abs_path.clone();
            list = list.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(Hsla { a: 0.1, ..c.danger })
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(c.danger)
                    .child(div().flex_1().child(err))
                    .child(
                        div()
                            .id("rb-retry")
                            .rounded_md()
                            .border_1()
                            .border_color(c.edge)
                            .px_2()
                            .py_0p5()
                            .cursor_pointer()
                            .text_color(c.fg)
                            .hover(move |d| d.bg(c.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.remote_list(retry_path.clone(), cx)))
                            .child("Retry"),
                    ),
            );
        }
        if looks_like_path(&query) {
            list = list.child(
                div()
                    .px_2()
                    .py_1p5()
                    .text_xs()
                    .text_color(c.muted)
                    .child(format!("Press Enter to open {query} on {peer_name}")),
            );
        } else {
            if let Some(up) = parent.filter(|_| q.is_empty()) {
                list = list.child(
                    div()
                        .id("rb-up")
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .cursor_pointer()
                        .text_color(c.muted)
                        .hover(move |d| d.bg(c.hover).text_color(c.fg))
                        .on_click(cx.listener(move |this, _, window, cx| this.remote_go(up.clone(), window, cx)))
                        .child(Icon::new(IconName::ArrowUp).size(px(12.0)).text_color(c.subtle))
                        .child(".."),
                );
            }
            if loading && entries.is_none() {
                list = list.child(div().px_2().py_2().text_xs().text_color(c.subtle).child(format!("Asking {peer_name}…")));
            } else if let Some(entries) = entries {
                let shown: Vec<&RemoteEntry> = entries
                    .iter()
                    .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q))
                    .take(500)
                    .collect();
                if shown.is_empty() {
                    list = list.child(
                        div()
                            .px_2()
                            .py_3()
                            .text_xs()
                            .text_color(c.subtle)
                            .child(if q.is_empty() { "Empty folder" } else { "No matching files" }),
                    );
                } else {
                    for (i, entry) in shown.into_iter().enumerate() {
                        let full = connect_files::join_remote_path(&abs_path, &entry.name);
                        let is_dir = entry.dir;
                        let target_peer = peer.clone();
                        let icon = if is_dir { IconName::Folder } else { file_icon(&entry.name) };
                        let icon_color = if is_dir {
                            if entry.git { c.accent } else { c.muted }
                        } else {
                            c.subtle
                        };
                        let meta = if is_dir {
                            entry.git.then(|| "git".to_string())
                        } else {
                            Some(format_size(entry.size))
                        };
                        list = list.child(
                            div()
                                .id(("rb-row", i))
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_md()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .cursor_pointer()
                                .text_color(c.fg)
                                .hover(move |d| d.bg(c.hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if is_dir {
                                        this.remote_go(full.clone(), window, cx);
                                    } else {
                                        this.open_remote(&target_peer, full.clone(), None, false, cx);
                                    }
                                }))
                                .child(Icon::new(icon).size(px(13.0)).text_color(icon_color))
                                .child(div().flex_1().min_w_0().truncate().child(entry.name.clone()))
                                .when_some(meta, |d, m| d.child(div().flex_shrink_0().text_size(px(10.5)).text_color(c.subtle).child(m))),
                        );
                    }
                }
                if truncated {
                    list = list.child(div().px_2().py_1().text_size(px(10.5)).text_color(c.subtle).child("Showing first 1,000 entries — type a path to jump directly."));
                }
            }
        }

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .children(switcher)
            .children(ws_chips)
            .child(search)
            .child(path_bar)
            .child(list)
            .into_any_element()
    }
}
