//! Settings → Connect: pair with other winfer instances (desktops and servers), see their links,
//! unpair, and this instance's name and relay (docs/plans/winfer-connect.md).

use super::settings_harness::ago;
use super::settings_kit::{alpha, badge, banner, empty_state, icon_tile, panel, section};
use super::settings_system::{Look, button, result_banner};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use winfer_connect::pairing::Kind;
use winfer_connect::{PairStatus, PeerView, Status};

#[derive(Default)]
pub(super) struct ConnectPage {
    paste: Option<Entity<InputState>>,
    name: Option<Entity<InputState>>,
    relay: Option<Entity<InputState>>,
    result: Option<Result<String, String>>,
    /// A once-a-second repaint runs while a pair link counts down.
    ticking: bool,
    /// The incoming pair request last brought to the front (its code).
    prompted: Option<String>,
    /// The link being shown is for a server: show the install commands (`Some(docker)`).
    server: Option<bool>,
}

/// The relay's installer and the published image (native/crates/winferd).
const INSTALL_URL: &str = "https://winfer.danehub.com/install.sh";
const IMAGE: &str = "ghcr.io/rickydane/winferd";

/// The command that installs winferd and pairs it with this link.
pub(super) fn server_command(link: &str, docker: bool) -> String {
    if docker {
        format!("docker run -d --name winferd --restart unless-stopped -v winferd:/data -e WINFERD_PAIR='{link}' {IMAGE}")
    } else {
        format!("curl -fsSL {INSTALL_URL} | sh -s -- --pair '{link}'")
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// "LAN · 4 ms", "Relay · 80 ms", "Connecting…", "Offline · last seen 5 min ago".
pub(super) fn link_label(peer: &PeerView, now: u64) -> (String, bool) {
    match peer.link.via {
        Some(via) => {
            let via = if via == "lan" { "LAN" } else { "Relay" };
            (peer.link.rtt_ms.map_or(via.to_string(), |ms| format!("{via} · {ms} ms")), true)
        }
        None if peer.link.relay_connected => ("Waiting for it to come online".into(), false),
        None => {
            let seen = peer.last_seen.map(|t| format!(" · last seen {}", ago(t as f64, now as f64))).unwrap_or_default();
            (format!("Offline{seen}"), false)
        }
    }
}

impl Root {
    /// `connect://changed`: repaint the page if it's open; bring an incoming pair request forward.
    pub(super) fn on_connect_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let incoming = crate::connect::get().and_then(|c| match c.status().pairing {
            Some(PairStatus::Code { code, peer_name, incoming: true, confirmed: false, .. }) => Some((code, peer_name)),
            _ => None,
        });
        if let Some((code, name)) = incoming
            && self.connect_page.prompted.as_deref() != Some(code.as_str())
        {
            self.connect_page.prompted = Some(code.clone());
            self.execute(Command::Settings(Some("connect".into())), window, cx);
            if !status::window_focused() {
                let notification = serde_json::json!({ "paneId": "connect:pair", "title": format!("{name} wants to pair"), "body": format!("Check that both screens show {code}."), "kind": "info" });
                backend::background(move || {
                    if let Ok(n) = serde_json::from_value(notification) {
                        let _ = winfer_backend::notify::notify_action(backend::app().clone(), n);
                    }
                });
            }
        }
        let s = self.state.read(cx);
        if s.settings_open && s.settings_page == "connect" {
            cx.notify();
        }
    }

    /// A click on a Connect notification; false when it isn't one.
    pub(super) fn on_connect_notification(&mut self, payload: &serde_json::Value, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = payload["paneId"].as_str().unwrap_or_default();
        if id == "connect:pair" {
            self.execute(Command::Settings(Some("connect".into())), window, cx);
            return true;
        }
        false
    }

    fn connect_tick(&mut self, cx: &mut Context<Self>) {
        if self.connect_page.ticking {
            return;
        }
        self.connect_page.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let go = this.update(cx, |this, cx| {
                    let waiting = crate::connect::get().is_some_and(|c| matches!(c.status().pairing, Some(PairStatus::Waiting { .. })));
                    if !waiting {
                        this.connect_page.ticking = false;
                        return false;
                    }
                    cx.notify();
                    true
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn connect_inputs(&mut self, status: &Status, window: &mut Window, cx: &mut Context<Self>) {
        if self.connect_page.paste.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Pair link or host:port"));
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().to_string();
                    this.connect_join(&text, cx);
                }
                cx.notify();
            })
            .detach();
            self.connect_page.paste = Some(input);
        }
        if self.connect_page.name.is_none() {
            let name = status.name.clone();
            let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. })
                    && let Some(connect) = crate::connect::get()
                {
                    let name = input.read(cx).value().trim().to_string();
                    if !name.is_empty() && name != connect.status().name {
                        connect.rename(&name);
                        this.connect_page.result = Some(Ok(format!("Paired machines now see this one as {name}.")));
                    }
                }
            })
            .detach();
            self.connect_page.name = Some(input);
        }
        if self.connect_page.relay.is_none() {
            let url = status.relay_url.clone();
            let input = cx.new(|cx| InputState::new(window, cx).default_value(url));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            self.connect_page.relay = Some(input);
        }
    }

    fn connect_join(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(connect) = crate::connect::get() else { return };
        if text.trim().is_empty() {
            return;
        }
        let text = text.trim();
        // A link, or the address of a machine mDNS can't see (`host:port`).
        let looks_like_address = !text.contains("p=") && text.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok());
        let result = if looks_like_address { connect.pair_address(text) } else { connect.pair_join_link(text, false) };
        match result {
            Ok(()) => self.connect_page.result = None,
            Err(e) => self.connect_page.result = Some(Err(e)),
        }
        cx.notify();
    }

    pub(super) fn page_connect(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(connect) = crate::connect::get() else {
            return div().child(empty_state(c, IconName::Network, "Winfer Connect didn't start. See Diagnostics for the log."));
        };
        // Development aid: open on the "Add a server" panel.
        #[cfg(debug_assertions)]
        if std::env::var_os("WINFER_NATIVE_CONNECT_ADD_SERVER").is_some() && self.connect_page.server.is_none() && connect.status().pairing.is_none() {
            connect.pair_create_link();
            self.connect_page.server = Some(std::env::var("WINFER_NATIVE_CONNECT_ADD_SERVER").is_ok_and(|v| v == "docker"));
        }
        let status = connect.status();
        self.connect_inputs(&status, window, cx);
        let mono = crate::fonts::mono(cx);
        let now = now_ms();

        // What Connect does.
        let feature = |icon: IconName, text: &str| div().flex().items_center().gap_1p5().h(px(22.0)).px_2().rounded_md().bg(c.app).text_size(px(11.5)).text_color(c.muted).child(Icon::new(icon).size(px(12.0)).text_color(c.subtle)).child(text.to_string());
        let hero = panel(c).flex_row().items_center().gap_4().px_4().py_3p5()
            .child(icon_tile(c.accent, IconName::Network, 40.0))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Your other machines, here"))
                .child(div().text_size(px(12.0)).text_color(c.subtle).child("Pair another winfer (a desktop or a server) to open its sessions here, and yours there. Both sides get full control."))
                .child(div().flex().flex_wrap().gap_1p5()
                    .child(feature(IconName::SquareTerminal, "Live terminals"))
                    .child(feature(IconName::MessageSquare, "Chats"))
                    .child(feature(IconName::Bot, "Agents across machines"))
                    .child(feature(IconName::Bell, "Notifications"))))
            .child(div().flex_shrink_0().flex().flex_col().items_end().gap_2()
                .child(div().flex().items_center().gap_1().text_size(px(11.5)).text_color(c.success).child(Icon::new(IconName::Lock).size(px(11.0))).child("End-to-end encrypted")));

        // Pairing: start one, or the one under way.
        let pairing = match status.pairing.clone() {
            None => {
                let paste = self.connect_page.paste.clone().unwrap();
                let typed = paste.read(cx).value().trim().to_string();
                let join_from = paste.clone();
                panel(c).flex_row().gap_4().px_4().py_3p5()
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                        .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Pair from here"))
                        .child(div().text_size(px(12.0)).text_color(c.subtle).child("Make a one-time link and paste it into Settings → Connect on the other machine."))
                        .child(div().flex().gap_2()
                            .child(button(c, "connect-create", Some(IconName::Link), "Create pair link", Look::Primary, status.enabled).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(connect) = crate::connect::get() {
                                    connect.pair_create_link();
                                    this.connect_page.result = None;
                                    this.connect_page.server = None;
                                    this.connect_tick(cx);
                                }
                                cx.notify();
                            })))
                            .child(button(c, "connect-server", Some(IconName::Server), "Add a server…", Look::Plain, status.enabled).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(connect) = crate::connect::get() {
                                    connect.pair_create_link();
                                    this.connect_page.result = None;
                                    this.connect_page.server = Some(false);
                                    this.connect_tick(cx);
                                }
                                cx.notify();
                            })))))
                    .child(div().w(px(1.0)).bg(c.border))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                        .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Got a link?"))
                        .child(div().text_size(px(12.0)).text_color(c.subtle).child("Paste the pair link the other machine made, or its address on this network."))
                        .child(div().flex().items_center().gap_2()
                            .child(div().flex_1().min_w_0().font_family(mono.clone()).child(Input::new(&paste)))
                            .child(button(c, "connect-join", None, "Pair", Look::Primary, status.enabled && !typed.is_empty()).on_click(cx.listener(move |this, _, _, cx| {
                                let text = join_from.read(cx).value().to_string();
                                this.connect_join(&text, cx);
                            })))))
            }
            Some(PairStatus::Waiting { side, link, expires_at }) => {
                let left = expires_at.map(|x| x.saturating_sub(now) / 1000).unwrap_or(0);
                let countdown = div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(format!("expires in {}:{:02}", left / 60, left % 60));
                let cancel = button(c, "connect-cancel", None, "Cancel", Look::Plain, true).on_click(cx.listener(|_, _, _, cx| {
                    if let Some(connect) = crate::connect::get() {
                        connect.pair_cancel();
                    }
                    cx.notify();
                }));
                self.connect_tick(cx);
                if side == "host" && let Some(docker) = self.connect_page.server {
                    // Add a server: the command that installs winferd there and pairs it.
                    let command = server_command(&link.unwrap_or_default(), docker);
                    let copy = command.clone();
                    let tab = |id: &'static str, label: &'static str, on: bool, cx: &mut Context<Self>| {
                        div().id(id).px_2p5().h(px(24.0)).flex().items_center().rounded_md().cursor_pointer().text_size(px(12.0))
                            .when(on, |d| d.bg(c.app).text_color(c.fg)).when(!on, |d| d.text_color(c.subtle))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.connect_page.server = Some(id == "server-docker");
                                cx.notify();
                            }))
                    };
                    div().flex().flex_col().gap_3().p_4().rounded(px(12.0)).border_1().border_color(alpha(c.accent, 0.6)).bg(c.panel)
                        .child(div().flex().items_center().gap_2()
                            .child(Icon::new(IconName::Server).size(px(14.0)).text_color(c.accent))
                            .child(div().flex_1().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Add a server"))
                            .child(div().flex().gap_1().p_0p5().rounded_lg().bg(c.raised)
                                .child(tab("server-script", "Linux / macOS", !docker, cx))
                                .child(tab("server-docker", "Docker", docker, cx))))
                        .child(div().text_size(px(12.0)).text_color(c.subtle).child(if docker {
                            "Run this on the server. It pairs by itself; `docker logs winferd` shows the code. Add -e WINFERD_AGENTS=claude,codex to install agents."
                        } else {
                            "Run this on the server (over SSH). It installs winferd to ~/.local/bin, runs it as your user's service and prints the same code as here."
                        }))
                        .child(div().flex().items_center().gap_2().px_3().py_2().rounded_lg().bg(c.app)
                            .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(command))
                            .child(button(c, "connect-copy-server", Some(IconName::Copy), "Copy", Look::Primary, true).on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                this.toast("Command copied", cx);
                            }))))
                        .child(div().flex().items_center().gap_2().text_size(px(12.0)).text_color(c.muted)
                            .child(div().size(px(7.0)).rounded_full().bg(c.warning))
                            .child(div().flex_1().child("Waiting for the server…"))
                            .child(countdown)
                            .child(cancel))
                        .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.warning).child(Icon::new(IconName::KeyRound).size(px(12.0))).child("The command holds a one-time link with full control: only run it on your own servers."))
                } else if side == "host" {
                    let link = link.unwrap_or_default();
                    let copy = link.clone();
                    div().flex().flex_col().gap_3().p_4().rounded(px(12.0)).border_1().border_color(alpha(c.accent, 0.6)).bg(c.panel)
                        .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Paste this link on the other machine"))
                        .child(div().text_size(px(12.0)).text_color(c.subtle).child("Settings → Connect → Got a link?, or on a server: winferd pair '<link>'. Both screens then show a code to compare."))
                        .child(div().flex().items_center().gap_2().px_3().py_2().rounded_lg().bg(c.app)
                            .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_size(px(11.5)).text_color(c.muted).child(link))
                            .child(button(c, "connect-copy", Some(IconName::Copy), "Copy", Look::Primary, true).on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                this.toast("Pair link copied", cx);
                            }))))
                        .child(div().flex().items_center().gap_2().text_size(px(12.0)).text_color(c.muted)
                            .child(div().size(px(7.0)).rounded_full().bg(c.warning))
                            .child(div().flex_1().child("Waiting for the other machine…"))
                            .child(countdown)
                            .child(cancel))
                        .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.warning).child(Icon::new(IconName::KeyRound).size(px(12.0))).child("Whoever has this link can pair with full control: only paste it on your own machines."))
                } else {
                    panel(c).flex_row().items_center().gap_3().px_4().py_3p5()
                        .child(div().size(px(7.0)).rounded_full().bg(c.warning))
                        .child(div().flex_1().text_size(px(12.5)).text_color(c.fg).child("Reaching the other machine…"))
                        .child(countdown)
                        .child(cancel)
                }
            }
            Some(PairStatus::Code { code, peer_name, peer_kind, confirmed, incoming }) => {
                let what = if peer_kind == Kind::Server { "server" } else { "machine" };
                let title = if incoming { format!("The {what} {peer_name} wants to pair") } else { format!("Pair with the {what} {peer_name}?") };
                div().flex().items_center().gap_5().p_4().rounded(px(12.0)).border_1().border_color(alpha(c.accent, 0.6)).bg(c.panel)
                    .child(div().px_4().py_3().rounded_lg().bg(c.app).font_family(mono.clone()).text_size(px(30.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child(code))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title))
                        .child(div().text_size(px(12.0)).text_color(c.subtle).child(if peer_kind == Kind::Server {
                            "Check that the server printed the same code."
                        } else {
                            "Check that the other screen shows the same code, then confirm on both."
                        })))
                    .child(div().flex().flex_col().items_end().gap_2()
                        .child(button(c, "connect-confirm", Some(IconName::Check), if confirmed { "Waiting for the other side…" } else { "Codes match" }, Look::Primary, !confirmed).on_click(cx.listener(move |_, _, _, cx| {
                            if !confirmed && let Some(connect) = crate::connect::get() {
                                connect.pair_confirm();
                            }
                            cx.notify();
                        })))
                        .child(button(c, "connect-cancel", None, "They don't match", Look::Plain, true).on_click(cx.listener(|_, _, _, cx| {
                            if let Some(connect) = crate::connect::get() {
                                connect.pair_cancel();
                            }
                            cx.notify();
                        }))))
            }
            Some(PairStatus::Done { peer_name }) => result_banner(c, &Ok(format!("Paired with {peer_name}. Its sessions show in the sidebar under Connected.")), Some(
                button(c, "connect-dismiss", None, "OK", Look::Plain, true).on_click(cx.listener(|_, _, _, cx| {
                    if let Some(connect) = crate::connect::get() {
                        connect.pair_dismiss();
                    }
                    cx.notify();
                })).into_any_element(),
            )),
            Some(PairStatus::Failed { error }) => result_banner(c, &Err(error), Some(
                button(c, "connect-dismiss", None, "Try again", Look::Plain, true).on_click(cx.listener(|_, _, _, cx| {
                    if let Some(connect) = crate::connect::get() {
                        connect.pair_dismiss();
                    }
                    cx.notify();
                })).into_any_element(),
            )),
        };
        let result = self.connect_page.result.as_ref().map(|r| result_banner(c, r, None));

        // Unpaired instances on this network.
        let busy = status.pairing.as_ref().is_some_and(|p| !matches!(p, PairStatus::Done { .. } | PairStatus::Failed { .. }));
        let nearby = (!status.nearby.is_empty()).then(|| {
            let mut list = panel(c);
            for (i, n) in status.nearby.iter().enumerate() {
                let id = n.id.clone();
                let icon = if n.kind == Kind::Server { IconName::Server } else { IconName::Monitor };
                list = list.child(div().flex().items_center().gap_3().px_3p5().py_2().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(icon_tile(c.muted, icon, 26.0))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c.fg).child(n.name.clone()))
                    .child(button(c, SharedString::from(format!("connect-nearby-{id}")), Some(IconName::Link), "Pair", Look::Plain, !busy).on_click(cx.listener(move |this, _, _, cx| {
                        if busy {
                            return;
                        }
                        if let Some(connect) = crate::connect::get() {
                            this.connect_page.result = connect.pair_nearby(&id).err().map(Err);
                        }
                        cx.notify();
                    }))));
            }
            list
        });

        // The paired instances.
        let count = status.peers.len();
        let peers = if status.peers.is_empty() {
            panel(c).child(empty_state(c, IconName::Network, "No machines yet. Pair one above, or install winfer on a server and pair it the same way."))
        } else {
            let mut list = panel(c);
            for (i, peer) in status.peers.iter().enumerate() {
                let (label, up) = link_label(peer, now);
                let id = peer.peer_id.clone();
                let name = peer.name.clone();
                let notify = peer.notify;
                let notify_id = id.clone();
                let icon = if peer.kind == Kind::Server { IconName::Server } else { IconName::Monitor };
                list = list.child(div().flex().items_center().gap_3().px_3p5().py_2p5().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(icon_tile(if up { c.accent } else { c.muted }, icon, 30.0))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                        .child(div().flex().items_center().gap_2()
                            .child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(name.clone()))
                            .when(peer.kind == Kind::Server, |d| d.child(badge(c.muted, "Server"))))
                        .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle)
                            .child(div().size(px(7.0)).rounded_full().bg(if up { c.success } else { c.subtle }))
                            .child(label)
                            .when_some(peer.link.error.clone().filter(|_| !up), |d, e| d.child("·").child(div().truncate().text_color(c.danger).child(e)))))
                    .child(button(c, SharedString::from(format!("connect-notify-{id}")), Some(if notify { IconName::Bell } else { IconName::BellOff }), if notify { "Notifying" } else { "Muted" }, Look::Plain, true).on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(connect) = crate::connect::get() {
                            connect.set_notify(&notify_id, !notify);
                        }
                        cx.notify();
                    })))
                    .child(button(c, SharedString::from(format!("connect-unpair-{id}")), Some(IconName::Unlink), "Unpair", Look::Danger, true).on_click(cx.listener(move |this, _, window, cx| {
                        let target = id.clone();
                        this.confirm(&format!("Unpair {name}"), format!("{name} and this machine will no longer reach each other's sessions. Pair again to reconnect."), "Unpair", window, cx, move |this, cx| {
                            if let Some(connect) = crate::connect::get() {
                                connect.revoke(&target);
                            }
                            this.connect_page.result = None;
                            cx.notify();
                        });
                    }))));
            }
            list
        };

        // This instance.
        let name_input = self.connect_page.name.clone().unwrap();
        let relay = self.connect_page.relay.clone().unwrap();
        let typed = relay.read(cx).value().trim().trim_end_matches('/').to_string();
        let can_save = !typed.is_empty() && typed != status.relay_url;
        let save_from = relay.clone();
        let enabled = status.enabled;
        let discoverable = status.discoverable;
        let me = panel(c)
            .child(super::settings::row(c, "Connect", Some("Keeps this machine reachable by the machines it's paired with.".into()), self.toggle("connect-enabled", enabled, c, cx, move |_, cx| {
                if let Some(connect) = crate::connect::get() {
                    connect.set_enabled(!enabled);
                }
                cx.notify();
            })))
            .child(div().border_t_1().border_color(c.border).child(super::settings::row(c, "Name", Some("How paired machines see this one.".into()), div().w(px(240.0)).child(Input::new(&name_input)))))
            .child(div().border_t_1().border_color(c.border).child(super::settings::row(c, "Discoverable on this network", Some(match (status.lan_port, &status.lan_error) {
                (_, Some(e)) => format!("Machines on the same network can find this one and ask to pair; you still confirm a code. ({e})"),
                (Some(port), None) => format!("Machines on the same network can find this one and ask to pair; you still confirm a code. Port {port}."),
                (None, None) => "Machines on the same network can find this one and ask to pair; you still confirm a code.".to_string(),
            }), self.toggle("connect-discoverable", discoverable, c, cx, move |_, cx| {
                if let Some(connect) = crate::connect::get() {
                    connect.set_discoverable(!discoverable);
                }
                cx.notify();
            }))))
            .child(div().border_t_1().border_color(c.border).child(super::settings::row(c, "Relay URL", Some("Where new pairings meet when they aren't on the same network. Traffic is end-to-end encrypted.".into()),
                div().flex().items_center().gap_2()
                    .child(div().w(px(240.0)).font_family(mono).child(Input::new(&relay)))
                    .child(button(c, "connect-relay-save", None, "Save", Look::Primary, can_save).on_click(cx.listener(move |this, _, _, cx| {
                        if !can_save {
                            return;
                        }
                        let value = save_from.read(cx).value().to_string();
                        if let Some(connect) = crate::connect::get() {
                            this.connect_page.result = Some(connect.set_relay_url(&value).map(|_| "Relay saved. New pairings meet there; existing ones stay where they met.".into()));
                        }
                        cx.notify();
                    }))))));

        div().flex().flex_col().gap_4()
            .child(hero)
            .when(!enabled, |d| d.child(banner(c.warning, IconName::TriangleAlert, "Connect is off: paired machines can't reach this one.")))
            .child(pairing)
            .children(result)
            .when_some(nearby, |d, list| d.child(section(c, "On this network", Some("winfer instances you haven't paired".into()))).child(list))
            .child(section(c, "Paired machines", Some(format!("{count} machine{}", if count == 1 { "" } else { "s" }))))
            .child(peers)
            .child(section(c, "This machine", None))
            .child(me)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_commands_carry_the_link() {
        let link = "winfer-connect:#p=abc";
        assert_eq!(super::server_command(link, false), "curl -fsSL https://winfer.danehub.com/install.sh | sh -s -- --pair 'winfer-connect:#p=abc'");
        assert!(super::server_command(link, true).contains("-e WINFERD_PAIR='winfer-connect:#p=abc' ghcr.io/rickydane/winferd"));
    }
}
