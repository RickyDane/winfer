//! Settings → winfer code and Mobile (port of `HarnessSettings` and `MobileSettings`).

use super::settings_kit::{alpha, badge, empty_state, icon_tile, panel, section};
use super::settings_system::{Look, button, result_banner};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use winfer_backend::{harness, mobile};

static HINTS: LazyLock<HashMap<String, String>> = LazyLock::new(|| serde_json::from_str(include_str!("../../assets/settings-hints.json")).unwrap_or_default());

pub(super) fn hint(key: &str) -> Option<String> {
    HINTS.get(key).cloned()
}

pub(super) fn card_description(title: &str) -> String {
    HINTS.get(&format!("card:{title}")).cloned().unwrap_or_default()
}

pub(super) const BACKENDS: [(&str, &str); 4] = [("chatgpt", "ChatGPT account"), ("claude", "Claude Code"), ("codex", "Codex"), ("local", "Local model")];
pub(super) const WINFER_PROVIDER: &str = "winfer";

/// "just now", "5 min ago", "3 h ago", "2 d ago".
pub fn ago(at: f64, now: f64) -> String {
    let s = ((now - at) / 1000.0).round().max(0.0);
    if s < 60.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{} min ago", (s / 60.0).round())
    } else if s < 86400.0 {
        format!("{} h ago", (s / 3600.0).round())
    } else {
        format!("{} d ago", (s / 86400.0).round())
    }
}

/// Splits the always-allowed commands field on commas and whitespace.
pub fn split_commands(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// The MCP servers field: empty is none, otherwise it must be a JSON object.
pub fn parse_servers(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let parsed: Value = serde_json::from_str(text).map_err(|e| format!("MCP servers: {e}"))?;
    if parsed.is_object() { Ok(parsed) } else { Err("MCP servers: expected an object of servers".into()) }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

#[derive(Default)]
pub(super) struct HarnessPage {
    pub(super) settings: Option<Value>,
    pub(super) models: Option<Vec<Value>>,
    pub(super) skills: Option<Vec<Value>>,
    /// Adds an always-allowed command (Enter or leaving the field).
    pub(super) commands: Option<Entity<InputState>>,
    pub(super) mcp: Option<Entity<TextareaState>>,
    pub(super) result: Option<Result<String, String>>,
    status: Option<Value>,
    pairing: Option<Value>,
    qr: Option<Arc<Image>>,
    relay: Option<Entity<InputState>>,
    mobile_result: Option<Result<String, String>>,
    relay_result: Option<Result<String, String>>,
    polled: Option<std::time::Instant>,
    pub(super) local: Option<LocalInputs>,
    pub(super) local_test: Option<Result<String, String>>,
    pub(super) local_testing: bool,
    /// The backend "Automatic" runs on (`chatgpt`, `claude` or `codex`), looked up on load.
    pub(super) auto_backend: Option<String>,
    /// The page's tab, the skill picker and the other state of its controls.
    pub(super) ui: super::settings_code::CodeUi,
}

/// The text fields of Settings → winfer code → Local models. Edits stay in the fields until one
/// loses focus, so typing a host doesn't refetch models on every key.
pub(super) struct LocalInputs {
    pub(super) host: Entity<InputState>,
    pub(super) port: Entity<InputState>,
    pub(super) path: Entity<InputState>,
    pub(super) key: Entity<InputState>,
    pub(super) context: Entity<InputState>,
    pub(super) efforts: Entity<InputState>,
    pub(super) budgets: HashMap<String, Entity<InputState>>,
}

pub(super) fn effort_label(level: &str) -> String {
    match level {
        "xhigh" => "Extra high".into(),
        _ => level.chars().next().map_or(String::new(), |c| c.to_uppercase().chain(level.chars().skip(1)).collect()),
    }
}

/// Comma- or space-separated thinking levels, lowercased and without repeats.
pub fn parse_efforts(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in text.split(|c: char| c == ',' || c.is_whitespace()).map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

pub(super) fn digits(text: &str) -> u64 {
    text.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

impl Root {
    pub(super) fn load_harness(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.harness_page.settings.is_some() {
            return;
        }
        let settings = serde_json::to_value(harness::harness_settings()).unwrap_or_default();
        let mcp_text = if settings["mcpServers"].as_object().is_some_and(|m| !m.is_empty()) { serde_json::to_string_pretty(&settings["mcpServers"]).unwrap_or_default() } else { String::new() };
        let commands = cx.new(|cx| InputState::new(window, cx).placeholder("Add…"));
        let mcp = cx.new(|cx| TextareaState::new(window, cx).auto_grow(14, 24).placeholder("{}").default_value(mcp_text));
        // Typing a command and pressing Enter (or leaving the field) adds it to the list.
        cx.subscribe_in(&commands, window, |this, input, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                let typed = split_commands(&input.read(cx).value());
                if typed.is_empty() {
                    return;
                }
                let mut list = this.allowed_commands();
                for t in typed {
                    if !list.contains(&t) {
                        list.push(t);
                    }
                }
                this.save_harness("allowedCommands", json!(list), cx);
                input.update(cx, |s, cx| s.set_value("", window, cx));
            }
        })
        .detach();
        cx.subscribe(&mcp, |this, input, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Blur) {
                match parse_servers(&input.read(cx).value()) {
                    Ok(v) => this.save_harness("mcpServers", v, cx),
                    Err(e) => this.harness_page.result = Some(Err(e)),
                }
                cx.notify();
            }
        })
        .detach();
        self.harness_page.commands = Some(commands);
        self.harness_page.mcp = Some(mcp);
        self.harness_page.settings = Some(settings);
        self.harness_page.skills = None;
        cx.spawn(async move |this, cx| {
            let skills = backend::background(|| serde_json::to_value(harness::harness_claude_skills()).unwrap_or_default()).await.unwrap_or_default();
            let auto = backend::background(|| harness::backend::default_kind().id().to_string()).await.ok();
            let models = tauri::async_runtime::spawn(harness::harness_models()).await.ok().and_then(|m| serde_json::to_value(m).ok()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.harness_page.auto_backend = auto;
                this.harness_page.skills = Some(skills.as_array().cloned().unwrap_or_default());
                this.harness_page.models = Some(models.as_array().cloned().unwrap_or_default());
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn local_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.harness_page.local.is_some() {
            return;
        }
        let l = self.harness_page.settings.as_ref().map_or(Value::Null, |s| s["local"].clone());
        let text = |key: &str| l[key].as_str().unwrap_or_default().to_string();
        let number = |key: &str| l[key].as_u64().filter(|n| *n > 0).map_or(String::new(), |n| n.to_string());
        let make = |placeholder: &'static str, value: String, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(placeholder).default_value(value));
            cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.commit_local(None, Some(window), cx);
                }
            })
            .detach();
            input
        };
        let efforts = l["efforts"].as_array().map_or(String::new(), |a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
        self.harness_page.local = Some(LocalInputs {
            host: make("127.0.0.1", text("host"), false, window, cx),
            port: make("none", number("port"), false, window, cx),
            path: make("/v1", text("path"), false, window, cx),
            key: make("none", text("apiKey"), true, window, cx),
            context: make("Automatic", number("contextWindow"), false, window, cx),
            efforts: make("none", efforts, false, window, cx),
            budgets: HashMap::new(),
        });
    }

    /// The settings as the fields currently read, with `patch` applied.
    pub(super) fn local_draft(&self, patch: Option<(&str, Value)>, cx: &App) -> Value {
        let (Some(st), Some(inp)) = (self.harness_page.settings.as_ref(), self.harness_page.local.as_ref()) else { return Value::Null };
        let mut l = st["local"].clone();
        let get = |e: &Entity<InputState>| e.read(cx).value().to_string();
        l["host"] = json!(get(&inp.host).trim());
        l["port"] = json!(digits(&get(&inp.port)).min(65535));
        l["path"] = json!(get(&inp.path).trim());
        l["apiKey"] = json!(get(&inp.key).trim());
        l["contextWindow"] = json!(digits(&get(&inp.context)));
        l["efforts"] = json!(parse_efforts(&get(&inp.efforts)));
        let mut budgets = l["thinkingBudgets"].as_object().cloned().unwrap_or_default();
        for (level, input) in &inp.budgets {
            match digits(&get(input)) {
                0 => { budgets.remove(level); }
                n => { budgets.insert(level.clone(), json!(n)); }
            }
        }
        l["thinkingBudgets"] = Value::Object(budgets);
        if let Some((k, v)) = patch {
            l[k] = v;
        }
        l
    }

    pub(super) fn commit_local(&mut self, patch: Option<(&str, Value)>, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let draft = self.local_draft(patch, cx);
        if draft.is_null() {
            return;
        }
        if let (Some(inp), Some(window)) = (self.harness_page.local.as_ref(), window) {
            let efforts = draft["efforts"].as_array().map_or(String::new(), |a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
            inp.efforts.update(cx, |s, cx| if s.value() != efforts.as_str() { s.set_value(efforts, window, cx) });
        }
        if self.harness_page.settings.as_ref().is_some_and(|s| s["local"] != draft) {
            self.save_harness("local", draft, cx);
        }
    }

    pub(super) fn test_local(&mut self, cx: &mut Context<Self>) {
        let draft = self.local_draft(None, cx);
        let Ok(local) = serde_json::from_value::<harness::config::LocalLlm>(draft) else { return };
        self.harness_page.local_testing = true;
        self.harness_page.local_test = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = backend::background(move || winfer_backend::local_llm::list(&local)).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.harness_page.local_testing = false;
                this.harness_page.local_test = Some(result.and_then(|models| {
                    if models.is_empty() {
                        return Err("Connected, but the server lists no models.".to_string());
                    }
                    let list: Vec<String> = models.iter().map(|m| m.context.map_or(m.id.clone(), |c| format!("{} ({}k)", m.id, (c as f64 / 1000.0).round()))).collect();
                    Ok(format!("Connected · {}", list.join(", ")))
                }));
                cx.notify();
            });
        })
        .detach();
    }

    /// The always-allowed commands as saved.
    pub(super) fn allowed_commands(&self) -> Vec<String> {
        self.harness_page.settings.as_ref().and_then(|s| s["allowedCommands"].as_array()).map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
    }

    pub(super) fn save_harness(&mut self, key: &str, value: Value, cx: &mut Context<Self>) {
        let Some(settings) = self.harness_page.settings.as_mut() else { return };
        settings[key] = value;
        let result = serde_json::from_value(settings.clone()).map_err(|e: serde_json::Error| e.to_string()).and_then(harness::harness_set_settings);
        self.harness_page.result = result.err().map(Err);
        cx.notify();
    }

    pub(super) fn harness_toggle(&self, key: &'static str, on: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        self.toggle(SharedString::from(format!("harness-{key}")), on, c, cx, move |this, cx| this.save_harness(key, json!(!on), cx))
    }

    pub(super) fn code_web(&self, key: &str, default: bool, cx: &App) -> bool {
        self.rest_bool("codeWeb", key, default, cx)
    }

    fn refresh_mobile(&mut self, cx: &mut Context<Self>) {
        self.harness_page.polled = Some(std::time::Instant::now());
        cx.spawn(async move |this, cx| {
            let status = backend::background(|| mobile::mobile_status().map(|s| serde_json::to_value(s).unwrap_or_default())).await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                if let Some(status) = status {
                    this.apply_mobile_status(status, cx);
                }
            });
        })
        .detach();
    }

    /// `mobile://changed`: the phones' list moved on.
    pub(super) fn on_mobile_changed(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).settings_open && self.state.read(cx).settings_page == "mobile" {
            self.refresh_mobile(cx);
        }
    }

    fn apply_mobile_status(&mut self, status: Value, cx: &mut Context<Self>) {
        let now = now_ms();
        if let Some(p) = self.harness_page.pairing.clone() {
            let device = status["devices"].as_array().and_then(|l| l.iter().find(|d| d["deviceId"] == p["deviceId"]));
            if device.is_some_and(|d| d["paired"] == true) {
                self.harness_page.mobile_result = Some(Ok(format!("Paired {}.", device.unwrap()["name"].as_str().unwrap_or("phone"))));
                self.harness_page.pairing = None;
                self.harness_page.qr = None;
            } else if now > p["expiresAt"].as_f64().unwrap_or(0.0) {
                self.harness_page.mobile_result = Some(Err("The pairing code expired. Make a new one.".into()));
                self.harness_page.pairing = None;
                self.harness_page.qr = None;
            }
        }
        self.harness_page.status = Some(status);
        cx.notify();
    }

    /// Makes a pairing code and ticks its countdown every second until it's used or expires.
    fn start_pairing(&mut self, cx: &mut Context<Self>) {
        self.harness_page.mobile_result = None;
        match mobile::mobile_pair_start().map(|p| serde_json::to_value(p).unwrap_or_default()) {
            Ok(p) => {
                self.harness_page.qr = p["qrSvg"].as_str().map(|svg| Arc::new(Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())));
                let device = p["deviceId"].clone();
                self.harness_page.pairing = Some(p);
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        let ticking = this.update(cx, |this, cx| {
                            let Some(p) = this.harness_page.pairing.as_ref().filter(|p| p["deviceId"] == device) else { return false };
                            if now_ms() > p["expiresAt"].as_f64().unwrap_or(0.0) {
                                this.harness_page.mobile_result = Some(Err("The pairing code expired. Make a new one.".into()));
                                this.harness_page.pairing = None;
                                this.harness_page.qr = None;
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(ticking, Ok(true)) {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(e) => self.harness_page.mobile_result = Some(Err(e)),
        }
        self.refresh_mobile(cx);
    }

    pub(super) fn page_mobile(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.harness_page.polled.is_none_or(|t| t.elapsed() > Duration::from_secs(15)) {
            self.refresh_mobile(cx);
        }
        if self.harness_page.relay.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            self.harness_page.relay = Some(input);
        }
        let relay = self.harness_page.relay.clone().unwrap();
        let status = self.harness_page.status.clone();
        if let Some(url) = status.as_ref().and_then(|s| s["relayUrl"].as_str())
            && relay.read(cx).value().is_empty()
        {
            let url = url.to_string();
            relay.update(cx, |i, cx| i.set_value(url, window, cx));
        }
        let mono = crate::fonts::mono(cx);
        let now = now_ms();
        let pairing = self.harness_page.pairing.clone();
        let devices: Vec<Value> = status.iter().flat_map(|s| s["devices"].as_array().cloned().unwrap_or_default()).collect();
        let pending = pairing.as_ref().and_then(|p| devices.iter().find(|d| d["deviceId"] == p["deviceId"]).cloned());

        // What the phone does, and the button that starts pairing.
        let phone_art = div().w(px(34.0)).h(px(56.0)).flex_shrink_0().flex().flex_col().gap(px(3.0)).p(px(4.0)).pt(px(8.0)).rounded(px(7.0)).border_2().border_color(c.border_strong).bg(c.app)
            .children([0.9, 0.7, 0.8].map(|w| div().h(px(5.0)).w(relative(w)).rounded_sm().bg(alpha(c.accent, 0.45))));
        let feature = |icon: IconName, text: &str| div().flex().items_center().gap_1p5().h(px(22.0)).px_2().rounded_md().bg(c.app).text_size(px(11.5)).text_color(c.muted).child(Icon::new(icon).size(px(12.0)).text_color(c.subtle)).child(text.to_string());
        let hero = panel(c).flex_row().items_center().gap_4().px_4().py_3p5()
            .child(phone_art)
            .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Your agents, in your pocket"))
                .child(div().flex().flex_wrap().gap_1p5()
                    .child(feature(IconName::Bell, "Notified when one finishes or asks"))
                    .child(feature(IconName::MessageSquare, "Read its thread"))
                    .child(feature(IconName::Reply, "Answer it"))))
            .child(div().flex_shrink_0().flex().flex_col().items_end().gap_2()
                .child(button(c, "mobile-pair", Some(IconName::QrCode), if pairing.is_some() { "New code" } else { "Pair a phone" }, Look::Primary, true).on_click(cx.listener(|this, _, _, cx| this.start_pairing(cx))))
                .child(div().flex().items_center().gap_1().text_size(px(11.5)).text_color(c.success).child(Icon::new(IconName::Lock).size(px(11.0))).child("End-to-end encrypted")));

        // The code to scan, the steps and how far the pairing is.
        let pairing_panel = pairing.map(|p| {
            let link = p["link"].as_str().unwrap_or_default().to_string();
            let (dot, waiting) = match &pending {
                Some(d) if d["connected"] == true => (c.success, "Waiting for your phone…".to_string()),
                Some(d) if d["error"].is_string() => (c.danger, format!("Can't reach the relay: {}", d["error"].as_str().unwrap_or_default())),
                _ => (c.warning, "Connecting to the relay…".to_string()),
            };
            let left = ((p["expiresAt"].as_f64().unwrap_or(0.0) - now) / 1000.0).max(0.0) as u64;
            let step = |n: &str, title: &str, sub: Option<&str>| div().flex().items_start().gap_2p5()
                .child(div().size(px(20.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().bg(alpha(c.accent, 0.16)).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.accent).child(n.to_string()))
                .child(div().flex().flex_col().gap_0p5().pt(px(1.0))
                    .child(div().text_size(px(12.5)).text_color(c.fg).child(title.to_string()))
                    .when_some(sub, |d, s| d.child(div().text_size(px(11.5)).text_color(c.subtle).child(s.to_string()))));
            div().flex().gap_4().p_4().rounded(px(12.0)).border_1().border_color(alpha(c.accent, 0.6)).bg(c.panel)
                .child(div().size(px(184.0)).flex_shrink_0().overflow_hidden().rounded_lg().bg(gpui_kit::white()).p_2().children(self.harness_page.qr.clone().map(|q| img(q).size_full())))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_3()
                    .child(step("1", "Scan the code with your phone's camera", None))
                    .child(step("2", "Open the page and add it to your Home Screen", None))
                    .child(step("3", "Allow notifications", Some("On iPhone they need iOS 16.4 or later and the Home Screen app.")))
                    .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.warning).child(Icon::new(IconName::KeyRound).size(px(12.0))).child("The code holds the key to your agents: don't share it. It works once."))
                    .child(div().flex_1())
                    .child(div().flex().items_center().gap_2().px_3().py_2().rounded_lg().bg(c.app).text_size(px(12.0)).text_color(c.muted)
                        .child(div().size(px(7.0)).flex_shrink_0().rounded_full().bg(dot))
                        .child(div().flex_1().min_w_0().truncate().child(waiting))
                        .child(div().flex_shrink_0().font_family(mono.clone()).text_size(px(11.0)).text_color(c.subtle).child(format!("expires in {}:{:02}", left / 60, left % 60)))
                        .child(button(c, "mobile-copy", Some(IconName::Link), "Copy link", Look::Plain, true).on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                            this.toast("Pairing link copied", cx);
                        })))))
        });
        let result = self.harness_page.mobile_result.as_ref().map(|r| result_banner(c, r, None));

        // The phones that can reach your agents.
        let paired: Vec<&Value> = devices.iter().filter(|d| d["paired"] == true).collect();
        let count = paired.len();
        let phones = if paired.is_empty() {
            panel(c).child(empty_state(c, IconName::Smartphone, "No phones yet. Pair one above to get notified when an agent finishes or asks something."))
        } else {
            let mut list = panel(c);
            for (i, d) in paired.into_iter().enumerate() {
                let open = d["phones"].as_u64().unwrap_or(0) > 0;
                let connected = d["connected"] == true;
                let seen = if open { "Open now".to_string() } else { d["lastSeen"].as_f64().map_or("Paired".to_string(), |t| format!("Last seen {}", ago(t, now))) };
                let push = d["push"] == true;
                let error = (!connected).then(|| d["error"].as_str().map(str::to_string)).flatten();
                let (id, name) = (d["deviceId"].as_str().unwrap_or_default().to_string(), d["name"].as_str().unwrap_or_default().to_string());
                let button_name = name.clone();
                list = list.child(div().flex().items_center().gap_3().px_3p5().py_2p5().when(i > 0, |d| d.border_t_1().border_color(c.border))
                    .child(icon_tile(c.muted, IconName::Smartphone, 30.0))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                        .child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(name))
                        .child(div().flex().items_center().gap_1p5().text_size(px(11.5)).text_color(c.subtle)
                            .child(div().size(px(7.0)).rounded_full().bg(if open { c.success } else if connected { c.subtle } else { c.danger }))
                            .child(seen)
                            .child("·")
                            .child(Icon::new(if push { IconName::Bell } else { IconName::BellOff }).size(px(11.0)))
                            .child(if push { "notifications on" } else { "notifications off" })
                            .when_some(error, |d, e| d.child("·").child(div().truncate().text_color(c.danger).child(format!("relay: {e}"))))))
                    .child(button(c, SharedString::from(format!("unpair-{id}")), Some(IconName::Unlink), "Unpair", Look::Danger, true).on_click(cx.listener(move |this, _, window, cx| {
                        let target = id.clone();
                        this.confirm(&format!("Unpair {button_name}"), format!("{button_name} will stop getting notifications and can't reach your agents until you pair it again."), "Unpair", window, cx, move |this, cx| {
                            if let Err(e) = mobile::mobile_revoke(target.clone()) {
                                this.harness_page.mobile_result = Some(Err(e));
                            }
                            this.refresh_mobile(cx);
                        });
                    }))));
            }
            list
        };

        // Where winfer and the phones meet.
        let url = status.as_ref().and_then(|s| s["relayUrl"].as_str()).unwrap_or_default().to_string();
        let default = status.as_ref().and_then(|s| s["defaultRelayUrl"].as_str()).unwrap_or_default().to_string();
        let typed = relay.read(cx).value().trim().trim_end_matches('/').to_string();
        let can_save = status.is_some() && typed != url;
        let save_from = relay.clone();
        let reset_input = relay.clone();
        let relay_state = if devices.iter().any(|d| d["connected"] == true) {
            Some(badge(c.success, "Connected"))
        } else if devices.iter().any(|d| d["error"].is_string()) {
            Some(badge(c.danger, "Can't reach it"))
        } else {
            None
        };
        let relay_card = panel(c).flex_row().items_start().gap_4().px_4().py_3p5()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Relay URL"))
                .child(div().text_size(px(12.0)).text_color(c.subtle).child(card_description("Relay"))))
            .child(div().w(px(300.0)).flex_shrink_0().flex().flex_col().gap_2()
                .child(div().font_family(mono).child(Input::new(&relay)))
                .child(div().flex().items_center().justify_end().gap_2()
                    .when(!default.is_empty() && url != default, |d| d.child(button(c, "relay-reset", Some(IconName::Undo2), "Reset to default", Look::Plain, true).on_click(cx.listener(move |this, _, window, cx| {
                        this.harness_page.relay_result = Some(mobile::mobile_set_relay_url(String::new()).map(|_| "Back on the default relay. Phones reconnect to it.".to_string()));
                        reset_input.update(cx, |i, cx| i.set_value("", window, cx));
                        this.refresh_mobile(cx);
                    }))))
                    .child(button(c, "relay-save", None, "Save", Look::Primary, can_save).on_click(cx.listener(move |this, _, _, cx| {
                        if !can_save {
                            return;
                        }
                        let value = save_from.read(cx).value().to_string();
                        this.harness_page.relay_result = Some(mobile::mobile_set_relay_url(value).map(|_| "Relay saved. Phones reconnect to it.".to_string()));
                        this.refresh_mobile(cx);
                    })))));
        let relay_result = self.harness_page.relay_result.as_ref().map(|r| result_banner(c, r, None));

        div().flex().flex_col().gap_4()
            .child(hero)
            .children(pairing_panel)
            .children(result)
            .child(section(c, "Paired phones", Some(format!("{count} phone{}", if count == 1 { "" } else { "s" }))))
            .child(phones)
            .child(div().flex().items_center().gap_2().mx(px(2.0)).mb(px(-6.0)).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted).child("Relay").child(div().flex_1()).children(relay_state))
            .child(relay_card)
            .children(relay_result)
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, digits, effort_label, parse_efforts, parse_servers, split_commands};

    #[test]
    fn ago_reads_like_the_tauri_app() {
        assert_eq!(ago(0.0, 30_000.0), "just now");
        assert_eq!(ago(0.0, 300_000.0), "5 min ago");
        assert_eq!(ago(0.0, 7_200_000.0), "2 h ago");
        assert_eq!(ago(0.0, 172_800_000.0), "2 d ago");
    }

    #[test]
    fn fields_parse() {
        assert_eq!(split_commands("git, cargo  pnpm"), ["git", "cargo", "pnpm"]);
        assert!(parse_servers("[]").is_err());
        assert_eq!(parse_servers(" ").unwrap(), serde_json::json!({}));
        assert_eq!(crate::ui::code_view::text_scale_step(1.0, 0.05), 1.05);
        assert_eq!(crate::ui::code_view::text_scale_step(1.5, 0.05), 1.5);
    }

    #[test]
    fn local_model_fields() {
        assert_eq!(parse_efforts("None, low  Medium,low,, xhigh"), ["none", "low", "medium", "xhigh"]);
        assert!(parse_efforts(" , ").is_empty());
        assert_eq!(digits("8 0a0"), 800);
        assert_eq!(effort_label("xhigh"), "Extra high");
        assert_eq!(effort_label("low"), "Low");
    }
}
