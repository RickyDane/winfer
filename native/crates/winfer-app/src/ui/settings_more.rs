//! Settings → Voice and AI (the "visual + tabs" design in mockups/settings-overhaul/voice.html and
//! ai.html), plus the buttons and cards those pages and Accounts share.

use super::settings_kit::{PageTab, alpha, badge, icon_tile, panel, radio, section, tab_dot};
use super::*;
use crate::audio;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use winfer_backend::{chatgpt, chatgpt_voice, llm, realtime};

pub const LIVE_ROLE: &str = include_str!("../../assets/live-role.txt");
const DEFAULT_LIVE_MODEL: &str = "gpt-realtime";
const DEFAULT_LIVE_VOICE: &str = "marin";
const LIVE_VOICES: [&str; 10] = ["marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"];
const DEFAULT_CHATGPT_VOICE: &str = "cove";
const CHATGPT_VOICES: [&str; 9] = ["cove", "juniper", "maple", "spruce", "ember", "vale", "breeze", "arbor", "sol"];
/// The providers the AI can run on, in the order the automatic choice tries them.
const LLM_PROVIDERS: [(&str, &str); 3] = [("chatgpt", "ChatGPT"), ("codex", "Codex"), ("claude", "Claude Code")];
/// The input-level history the microphone test draws (one bar per sample).
const LEVEL_BARS: usize = 36;

fn title_case(v: &str) -> String {
    let mut chars = v.chars();
    chars.next().map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

/// What a live conversation would use with the current choice.
pub fn live_connection(auth: &str, has_key: bool, codex_ready: bool) -> Option<&'static str> {
    match auth {
        "apikey" => Some("api"),
        "chatgpt" => Some("chatgpt"),
        _ if has_key => Some("api"),
        _ if codex_ready => Some("chatgpt"),
        _ => None,
    }
}

/// The Voice tab a search hit leads to.
pub(super) fn voice_tab_for(label: &str) -> &'static str {
    match label {
        "Voice" | "System prompt" => "voice",
        "Input" | "Microphone" | "Read aloud" | "Speak agent updates aloud" => "mic",
        _ => "live",
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Btn {
    Plain,
    Primary,
    Ghost,
}

/// A button: bordered, filled with the accent, or quiet; with an optional icon.
pub(super) fn btn(c: Colors, id: impl Into<ElementId>, icon: Option<IconName>, label: impl Into<SharedString>, kind: Btn, enabled: bool) -> Stateful<Div> {
    let d = div().id(id).flex_shrink_0().h(px(26.0)).flex().items_center().gap_1p5().px_2p5().rounded_md().text_size(px(12.0));
    let d = match kind {
        Btn::Plain => d.border_1().border_color(c.edge).bg(c.raised).text_color(c.fg).when(enabled, |d| d.hover(move |d| d.bg(c.hover))),
        Btn::Primary => d.bg(c.accent).text_color(c.on_accent).font_weight(FontWeight::MEDIUM).when(enabled, |d| d.hover(|d| d.opacity(0.9))),
        Btn::Ghost => d.text_color(c.muted).when(enabled, |d| d.hover(move |d| d.bg(c.hover).text_color(c.fg))),
    };
    d.when(enabled, |d| d.cursor_pointer()).when(!enabled, |d| d.opacity(0.45)).when_some(icon, |d, i| d.child(Icon::new(i).size(px(13.0)))).child(label.into())
}

/// A short note with an icon and a bold first line.
pub(super) fn note_tile(c: Colors, icon: IconName, title: &str, text: &str) -> Div {
    panel(c).flex_1().min_w_0().flex_row().items_start().gap_2p5().px_3().py_2p5()
        .child(div().pt(px(1.0)).text_color(c.accent).child(Icon::new(icon).size(px(15.0))))
        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().text_size(px(12.0)).line_height(px(17.0))
            .child(div().font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()))
            .child(div().text_color(c.muted).child(text.to_string())))
}

/// A result as a badge in place (green when it worked, red with the error otherwise).
pub(super) fn result_badge(c: Colors, r: &Result<String, String>) -> AnyElement {
    match r {
        Ok(t) => badge(c.success, t.clone()).child(Icon::new(IconName::Check).size(px(11.0))).into_any_element(),
        Err(t) => div().min_w_0().text_size(px(11.5)).text_color(c.danger).child(t.clone()).into_any_element(),
    }
}

/// A row inside a card: label and hint on the left, the control on the right.
fn card_row(c: Colors, label: &str, hint: Option<String>, control: impl IntoElement) -> Div {
    div().flex().items_center().gap_4().px_4().py(px(11.0))
        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
            .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(label.to_string()))
            .when_some(hint, |d, h| d.child(div().text_size(px(12.0)).text_color(c.subtle).child(h))))
        .child(div().flex_shrink_0().flex().items_center().gap_2().child(control))
}

/// A card's header: icon tile, title and a line under it.
fn card_head(c: Colors, lead: impl IntoElement, title: &str, sub: &str) -> Div {
    div().flex().items_center().gap_2p5().px_4().py_3()
        .child(lead)
        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
            .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()))
            .child(div().text_size(px(11.5)).text_color(c.subtle).child(sub.to_string())))
}

/// An agent's brand mark on a tile.
pub(super) fn agent_mark(c: Colors, provider: &str, size: f32) -> Div {
    div().size(px(size)).flex_shrink_0().flex().items_center().justify_center().rounded(px(size * 0.27)).bg(c.hover).child(crate::ui::code_pickers::mark(c, provider, size * 0.55, 1.0))
}

struct Inputs {
    api_key: Entity<InputState>,
    live_model: Entity<InputState>,
    live_prompt: Entity<TextareaState>,
    writer_model: Entity<InputState>,
}

#[derive(Default)]
pub(super) struct MoreSettings {
    inputs: Option<Inputs>,
    has_key: Option<bool>,
    codex_ready: Option<bool>,
    voice_result: Option<Result<String, String>>,
    devices: Vec<String>,
    level: Arc<AtomicU32>,
    /// The input levels of the running (or last) microphone test, oldest first.
    levels: Vec<f32>,
    testing: bool,
    mic_result: Option<Result<String, String>>,
    signed_in: Option<(bool, Option<String>)>,
    models: Vec<String>,
    busy: Option<&'static str>,
    ai_result: Option<Result<String, String>>,
    /// Which voice list the Voice tab shows (true: the API key's); None follows the connection.
    api_voices: Option<bool>,
    /// The provider changed: empty the model field on the next render.
    writer_reset: bool,
}

impl Root {
    fn voice_str(&self, key: &str, cx: &App) -> Option<String> {
        self.state.read(cx).rest.get("voice").and_then(|v| v[key].as_str()).map(str::to_string)
    }

    fn more_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.more.inputs.is_some() {
            return;
        }
        let live_model_value = self.voice_str("liveModel", cx).unwrap_or_default();
        let prompt_value = self.voice_str("livePrompt", cx).unwrap_or_else(|| LIVE_ROLE.to_string());
        let writer_value = self.voice_str("writerModel", cx).unwrap_or_default();
        let api_key = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("sk-…"));
        let live_model = cx.new(|cx| InputState::new(window, cx).placeholder(DEFAULT_LIVE_MODEL).default_value(live_model_value));
        let live_prompt = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true).default_value(prompt_value));
        let writer_model = cx.new(|cx| InputState::new(window, cx).placeholder("CLI default").default_value(writer_value));
        cx.subscribe(&api_key, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&live_model, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                this.set_rest("voice", "liveModel", json!(input.read(cx).value().trim()), cx);
            }
        })
        .detach();
        cx.subscribe(&live_prompt, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Blur) {
                let text = input.read(cx).value().trim().to_string();
                this.set_rest("voice", "livePrompt", if text.is_empty() || text == LIVE_ROLE.trim() { Value::Null } else { json!(text) }, cx);
            }
        })
        .detach();
        cx.subscribe(&writer_model, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                this.set_rest("voice", "writerModel", json!(input.read(cx).value().trim()), cx);
            }
        })
        .detach();
        self.more.inputs = Some(Inputs { api_key, live_model, live_prompt, writer_model });
        self.refresh_more(cx);
    }

    fn refresh_more(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (key, ready, status, devices) = backend::background(|| {
                let status = serde_json::to_value(chatgpt::chatgpt_status()).unwrap_or_default();
                (realtime::realtime_has_api_key(), chatgpt_voice::chatgpt_voice_ready(), status, audio::input_devices())
            })
            .await
            .unwrap_or((false, false, Value::Null, Vec::new()));
            let signed = status["signedIn"] == true;
            let models = if signed { tauri::async_runtime::spawn(chatgpt::chatgpt_models()).await.ok().and_then(Result::ok).unwrap_or_default() } else { Vec::new() };
            let _ = this.update(cx, |this, cx| {
                this.more.has_key = Some(key);
                this.more.codex_ready = Some(ready);
                this.more.signed_in = Some((signed, status["email"].as_str().map(str::to_string)));
                this.more.models = models;
                this.more.devices = devices;
                cx.notify();
            });
        })
        .detach();
    }

    fn save_api_key(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let saved = key.is_some();
        let result = realtime::realtime_set_api_key(key);
        self.more.voice_result = Some(match result {
            Ok(()) => {
                self.more.has_key = Some(saved);
                if let Some(i) = &self.more.inputs {
                    i.api_key.update(cx, |s, cx| s.set_value("", window, cx));
                }
                Ok(if saved { "Saved".into() } else { "Removed".into() })
            }
            Err(e) => Err(e),
        });
        cx.notify();
    }

    fn test_mic(&mut self, cx: &mut Context<Self>) {
        self.more.testing = true;
        self.more.mic_result = None;
        self.more.levels.clear();
        let device = self.voice_str("inputDevice", cx);
        let level = self.more.level.clone();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(80)).await;
                let alive = this.update(cx, |this, cx| {
                    let now = f32::from_bits(this.more.level.load(Ordering::Relaxed));
                    this.more.levels.push(now);
                    if this.more.levels.len() > LEVEL_BARS {
                        this.more.levels.remove(0);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = backend::background(move || audio::test_microphone(device, level)).await.unwrap_or_else(|e| Err(e.to_string()));
            drop(poll);
            let _ = this.update(cx, |this, cx| {
                this.more.testing = false;
                this.more.mic_result = Some(match result {
                    Ok((name, peak)) if peak < 0.005 => Err(format!("Very quiet on “{name}”. Speak up or pick another microphone.")),
                    Ok((name, _)) => Ok(format!("Heard you on “{name}”")),
                    Err(e) => Err(e),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// A card to pick one of a few options: icon, title, a line on what it means and badges.
    #[allow(clippy::too_many_arguments)]
    fn choice_card(&self, c: Colors, id: String, on: bool, enabled: bool, lead: impl IntoElement, title: &str, text: &str, badges: Vec<AnyElement>, cx: &mut Context<Self>, pick: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> Stateful<Div> {
        div().id(SharedString::from(id)).flex_1().min_w_0().flex().flex_col().gap_2().p_3().rounded(px(10.0)).bg(c.panel)
            .border_1().border_color(if on { c.accent } else { c.edge })
            .when(on, |d| d.bg(alpha(c.accent, 0.06)))
            .when(enabled && !on, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover)))
            .when(!enabled, |d| d.opacity(0.55))
            .when(enabled, |d| d.on_click(cx.listener(move |this, _, _, cx| pick(this, cx))))
            .child(div().flex().items_center().gap_2()
                .child(lead)
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title.to_string()))
                .child(radio(c, on)))
            .child(div().flex_1().text_size(px(11.5)).line_height(px(16.0)).text_color(c.subtle).child(text.to_string()))
            .child(div().flex().flex_wrap().gap_1().children(badges))
    }

    // ───────────────────────── Voice ─────────────────────────

    fn voice_ready(&self, cx: &App) -> bool {
        let auth = self.voice_str("liveAuth", cx).unwrap_or_else(|| "auto".into());
        let (has_key, ready) = (self.more.has_key.unwrap_or(false), self.more.codex_ready.unwrap_or(false));
        match live_connection(&auth, has_key, ready) {
            Some("api") => has_key,
            Some(_) => ready,
            None => false,
        }
    }

    pub(super) fn voice_tabs(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let ready = self.more.has_key.is_none() || self.voice_ready(cx);
        let tabs = vec![
            PageTab::new("live", "Live voice", IconName::AudioWaveform).extra(Some(tab_dot(if ready { c.success } else { c.warning }))),
            PageTab::new("voice", "Voice & behaviour", IconName::User),
            PageTab::new("mic", "Microphone & read aloud", IconName::Mic),
        ];
        self.page_tab_bar("voice", "live", tabs, c, cx)
    }

    pub(super) fn page_voice(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.more_inputs(window, cx);
        let body = match self.page_tab("voice", "live") {
            "voice" => self.voice_behaviour(c, cx),
            "mic" => self.voice_mic(c, cx),
            _ => self.voice_live(c, cx),
        };
        body.pt(px(20.0))
    }

    fn voice_live(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let (api_key, live_model) = {
            let i = self.more.inputs.as_ref().unwrap();
            (i.api_key.clone(), i.live_model.clone())
        };
        let has_key = self.more.has_key.unwrap_or(false);
        let ready = self.more.codex_ready.unwrap_or(false);
        let auth = self.voice_str("liveAuth", cx).unwrap_or_else(|| "auto".into());
        let uses = live_connection(&auth, has_key, ready);
        let (status, ok) = match uses {
            Some("api") => (if has_key { "OpenAI API key" } else { "OpenAI API key · none saved" }, has_key),
            Some(_) => (if ready { "ChatGPT plan" } else { "ChatGPT plan · Codex isn't logged in" }, ready),
            None => ("Not set up yet", false),
        };
        let keys = commands::keys_for(&Command::LiveVoice, self.state.read(cx));

        let hero = panel(c).flex_row().items_center().gap_3p5().px_4().py_3p5()
            .child(icon_tile(c.accent, IconName::AudioWaveform, 38.0))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.fg).child("Talk with winfer"))
                .child(div().text_size(px(12.0)).text_color(c.subtle).child("A continuous conversation: it hears you, answers aloud and operates winfer."))
                .child(div().flex().flex_wrap().items_center().gap_1().text_size(px(12.0)).text_color(c.subtle)
                    .child("Start it with the wave button in the title bar")
                    .when_some(keys, |d, k| d.child("or").child(super::settings_kit::kbd(c, &mono, &k)))));

        let set_auth = |v: &'static str| move |this: &mut Self, cx: &mut Context<Self>| this.set_rest("voice", "liveAuth", json!(v), cx);
        let now_badge = match uses {
            Some("api") => badge(c.accent, "Now: API key"),
            Some(_) => badge(c.accent, "Now: ChatGPT plan"),
            None => badge(c.warning, "Needs a key or Codex login"),
        };
        let cards = div().flex().gap_3()
            .child(self.choice_card(c, "live-auth-auto".into(), auth == "auto", true, icon_tile(c.accent, IconName::WandSparkles, 26.0), "Automatic", "The API key when one is saved, else your ChatGPT plan.", vec![now_badge.into_any_element()], cx, set_auth("auto")))
            .child(self.choice_card(c, "live-auth-api".into(), auth == "apikey", true, icon_tile(c.muted, IconName::KeyRound, 26.0), "OpenAI API key", "OpenAI's realtime model operates winfer directly.", vec![badge(c.success, "Fastest").into_any_element(), badge(c.muted, "Billed to the API account").into_any_element()], cx, set_auth("apikey")))
            .child(self.choice_card(c, "live-auth-chatgpt".into(), auth == "chatgpt", true, agent_mark(c, "chatgpt", 26.0), "ChatGPT plan", "Its voice model hands each request to a model that runs winfer's tools.", vec![badge(c.muted, "Actions take a few seconds longer").into_any_element()], cx, set_auth("chatgpt")));

        let mut page = div().flex().flex_col().gap_4()
            .child(hero)
            .child(section(c, "Connection", None).child(div().flex().items_center().gap_1p5().font_weight(FontWeight::NORMAL).text_color(if ok { c.success } else { c.warning })
                .child(div().size(px(6.0)).rounded_full().bg(if ok { c.success } else { c.warning }))
                .child(format!("Uses: {status}"))))
            .child(cards);

        if auth != "chatgpt" {
            let can_save = !api_key.read(cx).value().trim().is_empty();
            let key_input = api_key.clone();
            let key_control = div().flex().items_center().gap_2()
                .when_some(self.more.voice_result.as_ref(), |d, r| d.child(result_badge(c, r)))
                .when(has_key && self.more.voice_result.is_none(), |d| d.child(badge(c.success, "Saved in the keychain")))
                .child(div().w(px(170.0)).child(Input::new(&api_key)))
                .child(btn(c, "key-save", None, if has_key { "Replace" } else { "Save" }, Btn::Plain, can_save).on_click(cx.listener(move |this, _, window, cx| {
                    let value = key_input.read(cx).value().trim().to_string();
                    if !value.is_empty() {
                        this.save_api_key(Some(value), window, cx);
                    }
                })))
                .when(has_key, |d| d.child(btn(c, "key-remove", None, "Remove", Btn::Ghost, true).on_click(cx.listener(|this, _, window, cx| this.save_api_key(None, window, cx)))));
            page = page.child(panel(c).overflow_hidden()
                .child(card_row(c, "OpenAI API key", Some("Create one at platform.openai.com → API keys. Stored in the keychain.".into()), key_control))
                .child(card_row(c, "Realtime model", Some(format!("Empty uses {DEFAULT_LIVE_MODEL}. Only the API key route uses it.")), div().w(px(190.0)).child(Input::new(&live_model))).border_t_1().border_color(c.border)));
        }
        if auth != "apikey" {
            page = page.child(panel(c).flex_row().items_center().gap_2p5().px_3().py_2p5().text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::Info).size(px(15.0)).text_color(c.accent))
                .child(div().flex_1().min_w_0().child("The ChatGPT plan always uses its own voice model, on the Codex CLI's login (codex login)."))
                .child(if ready { badge(c.success, "Codex is logged in") } else { badge(c.warning, "Codex isn't logged in yet") }));
        }
        page
    }

    fn voice_behaviour(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let mono = crate::fonts::mono(cx);
        let live_prompt = self.more.inputs.as_ref().unwrap().live_prompt.clone();
        let auth = self.voice_str("liveAuth", cx).unwrap_or_else(|| "auto".into());
        let uses_api = live_connection(&auth, self.more.has_key.unwrap_or(false), self.more.codex_ready.unwrap_or(false)) != Some("chatgpt");
        let api = self.more.api_voices.unwrap_or(uses_api);
        let (names, key, default): (&[&str], &'static str, &str) = if api { (&LIVE_VOICES, "liveVoice", DEFAULT_LIVE_VOICE) } else { (&CHATGPT_VOICES, "liveChatgptVoice", DEFAULT_CHATGPT_VOICE) };
        let chosen = self.voice_str(key, cx).unwrap_or_else(|| default.into());
        let list_seg = self.seg_control("voice-list", vec![(true, "API key voices"), (false, "ChatGPT plan voices")], api, c, cx, |this, v, cx| {
            this.more.api_voices = Some(v);
            cx.notify();
        });
        let options: Vec<(String, String)> = names.iter().map(|n| {
            let label = if *n == default { format!("{} (default)", title_case(n)) } else { title_case(n) };
            ((*n).to_string(), label)
        }).collect();
        let current_label = options.iter().find(|o| o.0 == chosen).map_or_else(|| title_case(&chosen), |o| o.1.clone());
        let voice_select = self.select_button("live-voice", current_label, options, c, cx, move |this, v, cx| {
            this.set_rest("voice", key, json!(v), cx);
        });
        let voice_card = panel(c).flex_row().items_center().gap_3().px_3p5().py_2p5()
            .child(div().flex_1().min_w_0().flex().items_center().gap_2p5()
                .child(list_seg)
                .child(div().truncate().text_size(px(11.5)).text_color(c.subtle).child(if api == uses_api { "the set your connection uses" } else { "not the set your connection uses" })))
            .child(voice_select);
        let custom = self.voice_str("livePrompt", cx).is_some();
        let reset_input = live_prompt.clone();
        let chip = |t: &str| div().px_1p5().rounded_md().bg(c.hover).text_size(px(10.5)).text_color(c.muted).child(t.to_string());
        let editor = panel(c).overflow_hidden()
            .child(div().flex().items_center().gap_2().px_3().py_2().border_b_1().border_color(c.border).text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::MessageSquareText).size(px(14.0)))
                .child("System prompt")
                .child(if custom { badge(c.accent, "Custom") } else { badge(c.muted, "Default") })
                .child(div().flex_1())
                .when(custom, |d| d.child(btn(c, "prompt-reset", Some(IconName::Undo2), "Reset to default", Btn::Ghost, true).on_click(cx.listener(move |this, _, window, cx| {
                    this.set_rest("voice", "livePrompt", Value::Null, cx);
                    reset_input.update(cx, |s, cx| s.set_value(LIVE_ROLE, window, cx));
                })))))
            .child(div().p_2p5().bg(c.app).font_family(mono.clone()).child(Textarea::new(&live_prompt).h(px(310.0)).appearance(false).text_size(px(12.5))))
            .child(div().flex().flex_wrap().items_center().gap_1p5().px_3().py_2().border_t_1().border_color(c.border).text_size(px(11.0)).text_color(c.subtle)
                .child(Icon::new(IconName::Plus).size(px(11.0)))
                .child("winfer adds after it:")
                .child(chip("current panes")).child(chip("workspaces")).child(chip("browser tabs"))
                .child(div().flex_1())
                .child(Icon::new(IconName::Zap).size(px(11.0)))
                .child("applies right away, mid-conversation too"));
        div().flex().flex_col().gap_4()
            .child(section(c, "Voice", Some("applies to the next conversation".into())))
            .child(voice_card)
            .child(section(c, "How it behaves", None))
            .child(editor)
    }

    fn voice_mic(&mut self, c: Colors, cx: &mut Context<Self>) -> Div {
        let selected = self.voice_str("inputDevice", cx).unwrap_or_default();
        let mut options = vec![(String::new(), "System default".to_string())];
        options.extend(self.more.devices.iter().map(|d| (d.clone(), d.clone())));
        if !selected.is_empty() && !self.more.devices.contains(&selected) {
            options.push((selected.clone(), format!("{selected} — not connected")));
        }
        let current = options.iter().find(|o| o.0 == selected).map_or_else(|| "System default".to_string(), |o| o.1.clone());
        let mic_select = self.select_button_flex("input-device", current, options, 300.0, false, c, cx, |this, v, cx| {
            this.more.mic_result = None;
            this.set_rest("voice", "inputDevice", if v.is_empty() { Value::Null } else { json!(v) }, cx);
        });
        let testing = self.more.testing;
        let mic = panel(c).flex_1().min_w_0().gap_2p5().p_3()
            .child(div().flex().items_center().gap_2().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(icon_tile(c.muted, IconName::Mic, 26.0)).child("Microphone"))
            .child(mic_select)
            .child(div().flex().items_center().gap_2()
                .child(btn(c, "mic-test", Some(IconName::AudioWaveform), if testing { "Listening…" } else { "Test" }, Btn::Plain, !testing).on_click(cx.listener(move |this, _, _, cx| {
                    if !testing {
                        this.test_mic(cx);
                    }
                })))
                .when_some(self.more.mic_result.as_ref(), |d, r| d.child(result_badge(c, r))));
        let mut bars = div().flex_1().flex().items_end().gap(px(2.0)).h(px(46.0)).px_2().pb_1().rounded(px(7.0)).bg(c.app);
        let levels = &self.more.levels;
        for i in 0..LEVEL_BARS {
            let v = levels.len().checked_sub(LEVEL_BARS - i).and_then(|j| levels.get(j)).copied();
            let h = v.map_or(2.0, |v| (v * 12.0).clamp(0.0, 1.0) * 38.0 + 2.0);
            bars = bars.child(div().flex_1().h(px(h)).rounded_full().bg(if v.is_some() { c.success } else { c.border_strong }));
        }
        let level = panel(c).w(px(230.0)).flex_shrink_0().gap_2().p_3()
            .child(div().flex().items_center().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Input level").child(div().flex_1())
                .child(div().text_size(px(11.0)).font_weight(FontWeight::NORMAL).text_color(c.subtle).child(if testing { "speak now" } else { "run a test" })))
            .child(bars);
        let readback = self.rest_bool("voice", "readback", false, cx);
        let read_toggle = self.toggle("readback", readback, c, cx, move |this, cx| this.set_rest("voice", "readback", json!(!readback), cx));
        let read_row = panel(c).flex_row().items_center().gap_2p5().px_3p5().py_2p5()
            .child(Icon::new(IconName::Volume2).size(px(15.0)).text_color(c.subtle))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                .child(div().flex().items_center().gap_2().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Speak agent updates aloud").children(self.reset_mark("voice.readback", c, cx)))
                .child(div().text_size(px(11.5)).text_color(c.subtle).child("A short sentence when an agent finishes or needs you.")))
            .child(read_toggle);
        let bubble = |provider: &str, text: &str| {
            div().flex().items_center().gap_2().pl_1().pr_3().py_1().rounded_full().bg(c.hover).text_size(px(12.0)).text_color(c.muted)
                .child(agent_mark(c, provider, 22.0))
                .child(Icon::new(IconName::Volume2).size(px(12.0)).text_color(c.accent))
                .child(text.to_string())
        };
        let examples = div().flex().flex_wrap().gap_2().p_3().rounded(px(10.0)).bg(c.app).when(!readback, |d| d.opacity(0.45))
            .child(bubble("codex", "“Codex needs your input”"))
            .child(bubble("claude", "“Claude Code finished”"));
        div().flex().flex_col().gap_4()
            .child(div().flex().gap_3().child(mic).child(level))
            .child(panel(c).flex_row().items_center().gap_2p5().px_3().py_2p5().text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::Info).size(px(15.0)).text_color(c.accent))
                .child(div().flex_1().min_w_0().child("A saved microphone that isn't plugged in stays in the list, marked “not connected”.")))
            .child(section(c, "Read aloud", Some("examples of what you'd hear".into())))
            .child(read_row)
            .child(examples)
    }

    // ───────────────────────── AI ─────────────────────────

    fn ai_action(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.more.busy = Some(name);
        self.more.ai_result = None;
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let result: Result<String, String> = match name {
                "signin" => tauri::async_runtime::spawn(chatgpt::chatgpt_sign_in(backend::app().clone())).await.map_err(|e| e.to_string()).and_then(|r| r).map(|_| "Signed in".to_string()),
                "signout" => backend::background(chatgpt::chatgpt_sign_out).await.unwrap_or_else(|e| Err(e.to_string())).map(|_| "Signed out".to_string()),
                _ => tauri::async_runtime::spawn(async {
                    let req = serde_json::from_value(json!({ "provider": "chatgpt", "model": "", "system": "Reply with OK.", "user": "ping" })).map_err(|e: serde_json::Error| e.to_string())?;
                    llm::llm_compose(req).await
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r)
                .map(|_| format!("Connected in {} ms", started.elapsed().as_millis())),
            };
            let _ = this.update(cx, |this, cx| {
                this.more.busy = None;
                this.more.ai_result = Some(result);
                this.refresh_more(cx);
            });
        })
        .detach();
    }

    pub(super) fn page_ai(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.more_inputs(window, cx);
        let writer_input = self.more.inputs.as_ref().unwrap().writer_model.clone();
        if std::mem::take(&mut self.more.writer_reset) {
            writer_input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let (signed_in, email) = self.more.signed_in.clone().unwrap_or((false, None));
        let busy = self.more.busy;
        let effective = self.llm_provider(cx).map(|p| p.0);
        let choice = self.voice_str("writer", cx).filter(|v| !v.is_empty());
        // The card that's picked: the explicit choice, else what the automatic order lands on.
        let picked = choice.clone().or_else(|| effective.clone()).unwrap_or_else(|| "off".into());
        let installed = |id: &str| self.state.read(cx).providers.iter().any(|p| p.id == id && p.installed);
        let (codex_ok, claude_ok) = (installed("codex"), installed("claude"));

        let uses = [
            (IconName::Loader, "Progress estimates", "How far each working agent is, on its pane"),
            (IconName::ListChecks, "Review summaries", "What an agent changed, on its review card"),
            (IconName::Type, "Titles & short texts", "Pane and chat titles, and the like"),
        ];
        let mut writes = div().flex().gap_3();
        for (icon, title, text) in uses {
            writes = writes.child(panel(c).flex_1().min_w_0().flex_row().items_start().gap_2p5().px_3().py_2p5()
                .child(icon_tile(c.accent, icon, 26.0))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(title))
                    .child(div().text_size(px(11.5)).text_color(c.subtle).child(text))));
        }

        let pick = |v: &'static str| {
            move |this: &mut Self, cx: &mut Context<Self>| {
                this.set_rest("voice", "writer", json!(v), cx);
                this.set_rest("voice", "writerModel", json!(""), cx);
                this.more.writer_reset = true;
            }
        };
        let state_badge = |ok: bool, yes: &str, no: &str| if ok { badge(c.success, yes.to_string()).into_any_element() } else { badge(c.warning, no.to_string()).into_any_element() };
        let chatgpt_lead = agent_mark(c, "chatgpt", 26.0);
        let cards = div().flex().gap_2p5()
            .child(self.choice_card(c, "llm-chatgpt".into(), picked == "chatgpt", true, chatgpt_lead, "ChatGPT", "Your Plus or Pro plan. No API key.", vec![state_badge(signed_in, "Signed in", "Sign in below")], cx, pick("chatgpt")))
            .child(self.choice_card(c, "llm-codex".into(), picked == "codex", codex_ok, agent_mark(c, "codex", 26.0), "Codex", "Headless, with your Codex login.", vec![state_badge(codex_ok, "Installed", "Not installed")], cx, pick("codex")))
            .child(self.choice_card(c, "llm-claude".into(), picked == "claude", claude_ok, agent_mark(c, "claude", 26.0), "Claude Code", "Headless, with your Claude login.", vec![state_badge(claude_ok, "Installed", "Not installed")], cx, pick("claude")))
            .child(self.choice_card(c, "llm-off".into(), picked == "off", true, icon_tile(c.muted, IconName::CircleOff, 26.0), "Off", "No progress estimates or review summaries.", vec![badge(c.muted, "Nothing runs").into_any_element()], cx, pick("off")));
        let order = if choice.is_none() {
            format!("Automatic: {}", effective.as_deref().and_then(|e| LLM_PROVIDERS.iter().find(|p| p.0 == e)).map_or("nothing available", |p| p.1))
        } else {
            "Automatic order: ChatGPT → Codex → Claude Code".to_string()
        };

        let detail = match picked.as_str() {
            "chatgpt" => {
                let account = if signed_in {
                    let initial = email.as_deref().and_then(|e| e.chars().next()).map_or("C".to_string(), |ch| ch.to_uppercase().to_string());
                    div().flex().items_center().gap_2p5().px_4().py_2p5().border_t_1().border_color(c.border)
                        .child(div().size(px(28.0)).flex_shrink_0().flex().items_center().justify_center().rounded_full().bg(alpha(c.success, 0.9)).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(c.on_accent).child(initial))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                            .child(div().truncate().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child(email.clone().unwrap_or_else(|| "ChatGPT account".into())))
                            .child(div().text_size(px(11.5)).text_color(c.subtle).child("signed in")))
                        .when_some(self.more.ai_result.as_ref(), |d, r| d.child(result_badge(c, r)))
                        .child(btn(c, "ai-test", Some(IconName::Activity), if busy == Some("test") { "Testing…" } else { "Test connection" }, Btn::Plain, busy.is_none()).on_click(cx.listener(move |this, _, _, cx| if busy.is_none() { this.ai_action("test", cx) })))
                        .child(btn(c, "ai-signout", None, "Sign out", Btn::Ghost, busy.is_none()).on_click(cx.listener(move |this, _, _, cx| if busy.is_none() { this.ai_action("signout", cx) })))
                } else {
                    let waiting = busy == Some("signin");
                    let fallback = effective.as_deref().filter(|e| *e != "chatgpt").and_then(|e| LLM_PROVIDERS.iter().find(|p| p.0 == e)).map(|p| p.1);
                    let sub = if waiting { "Finish signing in in your browser…".to_string() } else if let Some(f) = fallback { format!("Until you sign in, {f} writes them.") } else { "Until you sign in, nothing writes them.".to_string() };
                    div().flex().items_center().gap_2p5().px_4().py_2p5().border_t_1().border_color(c.border)
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
                            .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(c.fg).child("Not signed in"))
                            .child(div().text_size(px(11.5)).text_color(c.subtle).child(sub)))
                        .when_some(self.more.ai_result.as_ref(), |d, r| d.child(result_badge(c, r)))
                        .when(waiting, |d| d.child(btn(c, "ai-cancel", None, "Cancel", Btn::Ghost, true).on_click(cx.listener(|_, _, _, _| chatgpt::chatgpt_cancel_sign_in()))))
                        .child(btn(c, "ai-signin", None, "Continue with ChatGPT", Btn::Primary, busy.is_none()).on_click(cx.listener(move |this, _, _, cx| if busy.is_none() { this.ai_action("signin", cx) })))
                };
                let mut card = panel(c).overflow_hidden()
                    .child(card_head(c, agent_mark(c, "chatgpt", 30.0), "ChatGPT account", "Runs on your ChatGPT plan's usage. Set a weekly limit or disconnect winfer in ChatGPT's settings."))
                    .child(account);
                if signed_in && !self.more.models.is_empty() {
                    let chosen = self.voice_str("writerModel", cx).filter(|m| !m.is_empty()).unwrap_or_else(|| "gpt-6-luna".into());
                    let mut models = vec![("gpt-6-luna".to_string(), "gpt-6-luna (default)".to_string())];
                    models.extend(self.more.models.iter().filter(|m| *m != "gpt-6-luna").map(|m| (m.clone(), m.clone())));
                    let label = models.iter().find(|m| m.0 == chosen).map_or(chosen.clone(), |m| m.1.clone());
                    let model_select = self.select_button("llm-model", label, models, c, cx, |this, v, cx| this.set_rest("voice", "writerModel", json!(v), cx));
                    card = card.child(card_row(c, "Model", Some("From your plan's models.".into()), model_select).border_t_1().border_color(c.border));
                }
                card
            }
            id @ ("codex" | "claude") => {
                let (title, sub) = if id == "claude" {
                    ("Claude Code", "Runs Claude Code headless with your Claude login, without tools, MCP servers or your settings.")
                } else {
                    ("Codex CLI", "Runs Codex headless with your Codex login, read-only and without your Codex config.")
                };
                panel(c).overflow_hidden()
                    .child(card_head(c, agent_mark(c, id, 30.0), title, sub))
                    .child(card_row(c, "Model", Some("Empty uses the CLI's default model.".into()), div().w(px(224.0)).child(Input::new(&writer_input))).border_t_1().border_color(c.border))
            }
            _ => panel(c).flex_row().items_center().gap_2p5().px_3().py_2p5().text_size(px(12.0)).text_color(c.muted)
                .child(Icon::new(IconName::Info).size(px(15.0)).text_color(c.accent))
                .child(div().flex_1().min_w_0().child("Panes show no progress estimates and review cards no summaries until you pick a model above.")),
        };

        div().flex().flex_col().gap_4().pt_2()
            .child(section(c, "What it writes", None))
            .child(writes)
            .child(section(c, "Runs on", Some(order)))
            .child(cards)
            .child(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::{live_connection, voice_tab_for};

    #[test]
    fn live_connection_follows_the_choice() {
        assert_eq!(live_connection("auto", true, true), Some("api"));
        assert_eq!(live_connection("auto", false, true), Some("chatgpt"));
        assert_eq!(live_connection("auto", false, false), None);
        assert_eq!(live_connection("chatgpt", true, false), Some("chatgpt"));
        assert_eq!(live_connection("apikey", false, false), Some("api"));
    }

    #[test]
    fn search_hits_open_the_right_voice_tab() {
        assert_eq!(voice_tab_for("Microphone"), "mic");
        assert_eq!(voice_tab_for("System prompt"), "voice");
        assert_eq!(voice_tab_for("OpenAI API key"), "live");
    }
}
