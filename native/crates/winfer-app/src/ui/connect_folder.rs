//! "New workspace on ‹machine›": browse a paired machine's folders (`fs.list`) and open one as a
//! workspace there (`workspace.new`), or clone a git URL into the folder shown (`git.clone`).

use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use serde_json::{Value, json};

pub(super) struct FolderDialog {
    peer: String,
    name: String,
    input: Entity<InputState>,
    /// The folder shown (as the machine writes it, `~` for home).
    path: String,
    parent: Option<String>,
    dirs: Option<Vec<(String, bool)>>,
    error: Option<String>,
    /// The `fs.list` whose answer is awaited.
    listing: Option<String>,
    /// A deploy key request under way.
    pub key_request: Option<String>,
    /// A clone or workspace request under way.
    busy: Option<String>,
}

fn is_git_url(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("https://") || t.starts_with("http://") || t.starts_with("ssh://") || t.starts_with("git@") || t.starts_with("git://")) && !t.contains(char::is_whitespace)
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

impl Root {
    pub(super) fn open_remote_folder(&mut self, peer: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = crate::connect::peer_name(peer).unwrap_or_else(|| "that machine".into());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter, a folder path, or a git URL to clone here"));
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value().trim().to_string();
                this.folder_enter(&text, window, cx);
            }
            _ => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.connect.folder = Some(FolderDialog { peer: peer.into(), name, input, path: "~".into(), parent: None, dirs: None, error: None, listing: None, busy: None, key_request: None });
        self.folder_list("~", cx);
    }

    fn folder_list(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(peer) = self.connect.folder.as_ref().map(|d| d.peer.clone()) else { return };
        let id = self.connect_request(&peer, json!({"t": "fs.list", "path": path}), None);
        if let Some(d) = self.connect.folder.as_mut() {
            d.listing = Some(id);
            d.error = None;
        }
        cx.notify();
    }

    /// Enter: a git URL clones, a path opens that folder, a filter with one match goes into it.
    fn folder_enter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.connect.folder.as_ref() else { return };
        if is_git_url(text) {
            let parent = d.path.clone();
            return self.folder_clone(text, &parent, cx);
        }
        if text.starts_with('/') || text.starts_with('~') {
            return self.folder_list(text, cx);
        }
        let q = text.to_lowercase();
        let matches: Vec<String> = d.dirs.iter().flatten().filter(|(n, _)| n.to_lowercase().contains(&q)).map(|(n, _)| n.clone()).collect();
        if let [only] = &matches[..] {
            let path = join(&d.path, only);
            self.folder_go(&path, window, cx);
        } else if text.is_empty() {
            let path = d.path.clone();
            self.folder_open(&path, cx);
        }
    }

    fn folder_go(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.connect.folder.as_ref() {
            d.input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.folder_list(path, cx);
    }

    fn folder_open(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(peer) = self.connect.folder.as_ref().map(|d| d.peer.clone()) else { return };
        let id = self.connect_request(&peer, json!({"t": "workspace.new", "path": path}), None);
        if let Some(d) = self.connect.folder.as_mut() {
            d.busy = Some(id);
        }
        cx.notify();
    }

    fn folder_clone(&mut self, url: &str, parent: &str, cx: &mut Context<Self>) {
        let Some(peer) = self.connect.folder.as_ref().map(|d| d.peer.clone()) else { return };
        let id = self.connect_request(&peer, json!({"t": "git.clone", "url": url, "parent": parent}), None);
        if let Some(d) = self.connect.folder.as_mut() {
            d.busy = Some(id);
            d.error = None;
        }
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub(super) fn folder_clone_dev(&mut self, url: &str, cx: &mut Context<Self>) {
        self.folder_clone(url, "~", cx);
    }

    /// An answer for the dialog (`fs`, or the ack of its workspace/clone request); false if it isn't one.
    pub(super) fn on_folder_reply(&mut self, peer: &str, msg: &Value, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.connect.folder.as_mut().filter(|d| d.peer == peer) else { return false };
        let id = msg["id"].as_str().unwrap_or_default();
        if msg["t"] == "fs" && d.listing.as_deref() == Some(id) {
            d.listing = None;
            match msg["error"].as_str() {
                Some(e) => d.error = Some(e.to_string()),
                None => {
                    d.path = msg["path"].as_str().unwrap_or("~").to_string();
                    d.parent = msg["parent"].as_str().map(str::to_string);
                    d.dirs = Some(msg["dirs"].as_array().into_iter().flatten().map(|x| (x["name"].as_str().unwrap_or_default().to_string(), x["git"] == true)).collect());
                }
            }
            cx.notify();
            return true;
        }
        if msg["t"] == "server.key" || (msg["t"] == "ack" && d.key_request.as_deref() == Some(id)) {
            d.key_request = None;
            match msg["key"].as_str() {
                Some(key) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
                    self.toast("Deploy key copied. Add it to the repository (read access), then clone.", cx);
                }
                None => d.error = Some(msg["error"].as_str().unwrap_or("Couldn't get its deploy key").to_string()),
            }
            cx.notify();
            return true;
        }
        if msg["t"] == "ack" && d.busy.as_deref() == Some(id) {
            d.busy = None;
            if msg["ok"] == true {
                let name = d.name.clone();
                let what = msg["path"].as_str().map_or_else(|| "the workspace".to_string(), |p| p.to_string());
                self.connect.folder = None;
                self.toast(format!("New workspace on {name}: {what}. Start sessions there with its +."), cx);
                // Its mirror comes with the workspace in it.
                self.connect_request(peer, json!({"t": "panes.get"}), None);
            } else {
                d.error = Some(msg["error"].as_str().unwrap_or("it didn't work").to_string());
            }
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn render_remote_folder(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.connect.folder.as_ref()?;
        let c = *c;
        let query = d.input.read(cx).value().trim().to_string();
        let q = query.to_lowercase();
        let url = is_git_url(&query);
        let mut list = div().id("folder-list").overflow_y_scroll().max_h(px(340.0)).p_1p5().flex().flex_col();
        if let Some(parent) = d.parent.clone().filter(|_| !url) {
            list = list.child(
                div().id("folder-up").flex().items_center().gap_2().rounded_md().px_3().py_1p5().text_size(px(13.0)).cursor_pointer().text_color(c.muted).hover(move |s| s.bg(c.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.folder_go(&parent, window, cx)))
                    .child(Icon::new(IconName::ArrowUp).size(px(13.0)).text_color(c.subtle)).child(".."),
            );
        }
        if url {
            list = list.child(div().px_3().py_2().text_size(px(12.5)).text_color(c.muted).child(format!("Enter clones it into {} and opens it as a workspace.", d.path)));
            // A private repo over ssh needs the server's own key.
            if query.starts_with("git@") || query.starts_with("ssh://") {
                let name = d.name.clone();
                list = list.child(div().flex().items_center().gap_2().px_3().py_1p5().text_size(px(12.0)).text_color(c.subtle)
                    .child(div().flex_1().child(format!("Private? Add {name}'s deploy key to the repository first.")))
                    .child(div().id("folder-key").px_2().h(px(22.0)).flex().items_center().gap_1().rounded_md().border_1().border_color(c.border).cursor_pointer().text_color(c.fg).hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let Some(peer) = this.connect.folder.as_ref().map(|d| d.peer.clone()) else { return };
                            let id = this.connect_request(&peer, json!({"t": "server.key"}), None);
                            if let Some(d) = this.connect.folder.as_mut() {
                                d.key_request = Some(id);
                            }
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::KeyRound).size(px(11.0))).child("Copy deploy key")));
            }
        } else if d.listing.is_some() && d.dirs.is_none() {
            list = list.child(div().px_3().py_2().text_xs().text_color(c.subtle).child(format!("Asking {}…", d.name)));
        } else {
            for (i, (name, git)) in d.dirs.iter().flatten().filter(|(n, _)| q.is_empty() || n.to_lowercase().contains(&q)).enumerate().take(300) {
                let path = join(&d.path, name);
                list = list.child(
                    div().id(("folder-row", i)).flex().items_center().gap_2().rounded_md().px_3().py_1p5().text_size(px(13.0)).cursor_pointer().text_color(c.fg).hover(move |s| s.bg(c.hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.folder_go(&path, window, cx)))
                        .child(Icon::new(IconName::Folder).size(px(13.0)).text_color(if *git { c.accent } else { c.subtle }))
                        .child(div().flex_1().truncate().child(name.clone()))
                        .when(*git, |s| s.child(div().text_size(px(10.5)).text_color(c.subtle).child("git"))),
                );
            }
        }
        let (here, typed) = (d.path.clone(), query.clone());
        let busy = d.busy.is_some();
        let footer = div().flex().items_center().gap_2().px_3().py_2p5().border_t_1().border_color(c.border)
            .child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(if d.error.is_some() { c.danger } else { c.subtle })
                .child(d.error.clone().unwrap_or_else(|| if busy { "Working on it…".into() } else { format!("{} on {}", d.path, d.name) })))
            .child(div().id("folder-open").px_2p5().h(px(26.0)).flex().items_center().rounded_md().text_size(px(12.0)).cursor_pointer()
                .bg(c.accent).text_color(c.on_accent).when(busy, |s| s.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.connect.folder.as_ref().is_none_or(|d| d.busy.is_some()) {
                        return;
                    }
                    if url { this.folder_clone(&typed, &here, cx) } else { this.folder_open(&here, cx) }
                }))
                .child(if url { "Clone here" } else { "Open as workspace" }));
        let content = div().flex().flex_col()
            .child(div().px_4().pt_3().pb_1().flex().items_center().gap_2()
                .child(Icon::new(IconName::Server).size(px(13.0)).text_color(c.accent))
                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(format!("New workspace on {}", d.name))))
            // The field stands out from the list: a darker well with a search icon.
            .child(div().mx_3().mt_2().mb_1().h(px(38.0)).px_2p5().flex().items_center().gap_2().rounded_lg()
                .bg(if c.light { hsla(0.0, 0.0, 0.0, 0.05) } else { hsla(0.0, 0.0, 0.0, 0.28) }).border_1().border_color(c.edge)
                .child(Icon::new(IconName::Search).size(px(14.0)).text_color(c.subtle))
                .child(div().flex_1().min_w_0().child(Input::new(&d.input).appearance(false))))
            .child(list)
            .child(footer);
        Some(
            div().id("folder-backdrop").absolute().size_full().flex().items_start().justify_center().pt(px(110.0)).bg(hsla(0.0, 0.0, 0.0, 0.4)).occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.connect.folder = None;
                    cx.notify();
                }))
                .child(div().id("folder").w(px(520.0)).h_auto().overflow_hidden().rounded_xl().border_1().border_color(c.edge).bg(c.raised).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(content))
                .into_any_element(),
        )
    }
}
