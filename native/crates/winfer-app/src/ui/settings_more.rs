//! Settings → Voice and AI (port of those pages in `features/settings/SettingsModal.tsx`).

use super::review::small_button;
use super::settings::{card, plain_button, row, stacked_row};
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
const LLM_PROVIDERS: [(&str, &str); 3] = [("chatgpt", "ChatGPT (your plan, Sign in with ChatGPT)"), ("codex", "Codex CLI (your Codex login)"), ("claude", "Claude Code (your Claude login)")];

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
    testing: bool,
    mic_result: Option<Result<String, String>>,
    signed_in: Option<(bool, Option<String>)>,
    models: Vec<String>,
    busy: Option<&'static str>,
    ai_result: Option<Result<String, String>>,
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
        let live_prompt = cx.new(|cx| TextareaState::new(window, cx).auto_grow(8, 16).soft_wrap(true).default_value(prompt_value));
        let writer_model = cx.new(|cx| InputState::new(window, cx).default_value(writer_value));
        cx.subscribe(&api_key, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&live_model, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
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
            if matches!(event, InputEvent::Blur) {
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
                Ok(if saved { "API key saved in the keychain.".into() } else { "API key removed.".into() })
            }
            Err(e) => Err(e),
        });
        cx.notify();
    }

    fn test_mic(&mut self, cx: &mut Context<Self>) {
        self.more.testing = true;
        self.more.mic_result = None;
        let device = self.voice_str("inputDevice", cx);
        let level = self.more.level.clone();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
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
                    Ok((name, _)) => Ok(format!("Heard you on “{name}”.")),
                    Err(e) => Err(e),
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn page_voice(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.more_inputs(window, cx);
        let (api_key, live_model, live_prompt) = {
            let i = self.more.inputs.as_ref().unwrap();
            (i.api_key.clone(), i.live_model.clone(), i.live_prompt.clone())
        };
        let has_key = self.more.has_key.unwrap_or(false);
        let ready = self.more.codex_ready.unwrap_or(false);
        let auth = self.voice_str("liveAuth", cx).unwrap_or_else(|| "auto".into());
        let uses = live_connection(&auth, has_key, ready);
        let (status, ok) = match uses {
            Some("api") => (if has_key { "OpenAI API key" } else { "OpenAI API key (none saved)" }, has_key),
            Some(_) => (if ready { "ChatGPT plan (Codex login)" } else { "ChatGPT plan (Codex isn't logged in)" }, ready),
            None => ("Not set up", false),
        };
        let auths = [("auto", "Automatic (API key, else ChatGPT)"), ("apikey", "OpenAI API key"), ("chatgpt", "ChatGPT plan")];
        let auth_label = auths.iter().find(|a| a.0 == auth).map_or(auths[0].1, |a| a.1).to_string();
        let auth_select = self.select_button("live-auth", auth_label, auths.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(), c, cx, |this, v, cx| this.set_rest("voice", "liveAuth", json!(v), cx));
        let can_save = !api_key.read(cx).value().trim().is_empty();
        let key_input = api_key.clone();
        let key_row = div().flex().items_center().gap_2().child(div().w(px(192.0)).child(Input::new(&api_key)))
            .child(plain_button(c, "key-save", "Save").when(!can_save, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, window, cx| {
                let value = key_input.read(cx).value().trim().to_string();
                if !value.is_empty() {
                    this.save_api_key(Some(value), window, cx);
                }
            })))
            .when(has_key, |d| d.child(plain_button(c, "key-remove", "Remove").on_click(cx.listener(|this, _, window, cx| this.save_api_key(None, window, cx)))));
        let voice = self.voice_str("liveVoice", cx).unwrap_or_else(|| DEFAULT_LIVE_VOICE.into());
        let chatgpt_voice_name = self.voice_str("liveChatgptVoice", cx).unwrap_or_else(|| DEFAULT_CHATGPT_VOICE.into());
        let list = |names: &[&str]| names.iter().map(|v| (v.to_string(), title_case(v))).collect::<Vec<_>>();
        let voice_select = self.select_button("live-voice", title_case(&voice), list(&LIVE_VOICES), c, cx, |this, v, cx| this.set_rest("voice", "liveVoice", json!(v), cx));
        let chatgpt_select = self.select_button("live-chatgpt-voice", title_case(&chatgpt_voice_name), list(&CHATGPT_VOICES), c, cx, |this, v, cx| this.set_rest("voice", "liveChatgptVoice", json!(v), cx));
        let custom = self.voice_str("livePrompt", cx).is_some();
        let reset_input = live_prompt.clone();
        let prompt = div().flex().flex_col().gap_2().child(Textarea::new(&live_prompt)).when(custom, |d| d.child(div().child(plain_button(c, "prompt-reset", "Reset to default").on_click(cx.listener(move |this, _, window, cx| {
            this.set_rest("voice", "livePrompt", Value::Null, cx);
            reset_input.update(cx, |s, cx| s.set_value(LIVE_ROLE, window, cx));
        })))));
        let keys = commands::keys_for(&Command::Unavailable("Live voice"), self.state.read(cx)).unwrap_or_default();
        let live = card(c, "Live voice", Some(&format!("Talk with winfer in a continuous conversation ({keys} or the wave button in the title bar). An OpenAI voice model hears you, answers aloud and operates winfer.")))
            .child(row(c, "Connection", Some(format!("Uses: {status}{}. An API key runs OpenAI's realtime model, which operates winfer directly (fastest; billed to the API account). The ChatGPT plan runs its voice model on the Codex CLI's login (`codex login`); it hands each request to a model that runs winfer's tools, so actions take a few seconds longer.", if ok { "" } else { " (needs setup)" })), auth_select))
            .child(row(c, "OpenAI API key", Some(if has_key { "Stored in the keychain.".to_string() } else { "Create one at platform.openai.com → API keys.".to_string() }), key_row))
            .child(row(c, "Model (API key)", Some(format!("Empty uses {DEFAULT_LIVE_MODEL}. The ChatGPT plan always uses its own voice model.")), div().w(px(192.0)).child(Input::new(&live_model))))
            .child(stacked_row(c, "System prompt", Some("How the voice agent behaves and talks. winfer adds the current panes, workspaces and browser tabs after it; changes apply right away, also mid-conversation.".to_string()), prompt))
            .child(row(c, "Voice", Some("API key · ChatGPT plan. Applies to the next conversation.".to_string()), div().flex().flex_col().gap_1().child(voice_select).child(chatgpt_select)));
        let selected = self.voice_str("inputDevice", cx).unwrap_or_default();
        let mut options = vec![(String::new(), "System default".to_string())];
        options.extend(self.more.devices.iter().map(|d| (d.clone(), d.clone())));
        if !selected.is_empty() && !self.more.devices.contains(&selected) {
            options.push((selected.clone(), format!("{selected} — not connected")));
        }
        let current = if selected.is_empty() { "System default".to_string() } else { selected };
        let mic_select = self.select_button("input-device", current, options, c, cx, |this, v, cx| {
            this.more.mic_result = None;
            this.set_rest("voice", "inputDevice", if v.is_empty() { Value::Null } else { json!(v) }, cx);
        });
        let testing = self.more.testing;
        let level = f32::from_bits(self.more.level.load(Ordering::Relaxed));
        let hint = self.more.mic_result.as_ref().map(|r| r.as_ref().map_or_else(|e| e.clone(), |o| o.clone()));
        let mic = card(c, "Input", None).child(row(c, "Microphone", hint, div().flex().flex_col().items_end().gap_1()
            .child(div().flex().items_center().gap_2().child(mic_select).child(small_button(c, "mic-test", if testing { "Listening…" } else { "Test" }).on_click(cx.listener(move |this, _, _, cx| if !testing { this.test_mic(cx) }))))
            .when(testing, |d| d.child(div().h(px(6.0)).w(px(160.0)).overflow_hidden().rounded_full().bg(c.hover).child(div().h_full().bg(c.success).w(relative((level * 12.0).min(1.0))))))));
        let readback = self.rest_bool("voice", "readback", false, cx);
        let read_toggle = self.toggle("readback", readback, c, cx, move |this, cx| this.set_rest("voice", "readback", json!(!readback), cx));
        let read = card(c, "Read aloud", None).child(row(c, "Speak agent updates aloud", Some("“Codex needs your input”, “Claude Code finished”.".to_string()), read_toggle));
        div().flex().flex_col().gap_6().child(live).children(self.more.voice_result.as_ref().map(|r| result_text(c, r))).child(mic).child(read)
    }

    fn ai_action(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.more.busy = Some(name);
        self.more.ai_result = None;
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let result: Result<String, String> = match name {
                "signin" => tauri::async_runtime::spawn(chatgpt::chatgpt_sign_in(backend::app().clone())).await.map_err(|e| e.to_string()).and_then(|r| r).map(|_| "Signed in".to_string()),
                "signout" => backend::background(|| chatgpt::chatgpt_sign_out()).await.unwrap_or_else(|e| Err(e.to_string())).map(|_| "Signed out".to_string()),
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
        let (signed_in, email) = self.more.signed_in.clone().unwrap_or((false, None));
        let busy = self.more.busy;
        let account = if signed_in {
            row(c, "Signed in", email, div().flex().gap_2()
                .child(plain_button(c, "ai-test", if busy == Some("test") { "Testing…" } else { "Test connection" }).on_click(cx.listener(move |this, _, _, cx| if busy.is_none() { this.ai_action("test", cx) })))
                .child(plain_button(c, "ai-signout", "Sign out").on_click(cx.listener(move |this, _, _, cx| if busy.is_none() { this.ai_action("signout", cx) }))))
        } else {
            let waiting = busy == Some("signin");
            row(c, "Not signed in", waiting.then(|| "Finish signing in in your browser…".to_string()), div().flex().gap_2()
                .when(waiting, |d| d.child(plain_button(c, "ai-cancel", "Cancel").on_click(cx.listener(|_, _, _, _| chatgpt::chatgpt_cancel_sign_in()))))
                .child(super::orchestration::accent_button(c, "ai-signin", "Continue with ChatGPT", busy.is_none()).on_click(cx.listener(move |this, _, _, cx| if busy.is_none() { this.ai_action("signin", cx) }))))
        };
        let current = self.llm_provider(cx).map(|p| p.0);
        let s = self.state.read(cx);
        let ready: Vec<&str> = LLM_PROVIDERS.iter().map(|p| p.0).filter(|id| if *id == "chatgpt" { signed_in } else { s.providers.iter().any(|p| p.id == *id && p.installed) }).collect();
        let mut options: Vec<(String, String)> = LLM_PROVIDERS.iter().map(|(id, label)| (id.to_string(), if ready.contains(id) { label.to_string() } else if *id == "chatgpt" { format!("{label} (sign in above)") } else { format!("{label} (not installed)") })).collect();
        options.retain(|(id, _)| ready.contains(&id.as_str()));
        options.push(("off".into(), "Off (no progress estimates or review summaries)".into()));
        let label = current.as_deref().and_then(|c| LLM_PROVIDERS.iter().find(|p| p.0 == c)).map_or("Off (no progress estimates or review summaries)".to_string(), |p| p.1.to_string());
        let select = self.select_button("llm-provider", label, options, c, cx, |this, v, cx| {
            this.set_rest("voice", "writer", json!(v), cx);
            this.set_rest("voice", "writerModel", json!(""), cx);
        });
        let hint = match current.as_deref() {
            Some("claude") => Some("Runs Claude Code headless with your Claude login, without tools, MCP servers or your settings.".to_string()),
            Some("codex") => Some("Runs Codex headless with your Codex login, read-only and without your Codex config.".to_string()),
            _ => None,
        };
        let mut llm_card = card(c, "LLM", Some("Writes progress estimates, review summaries and other short texts in winfer.")).child(row(c, "Provider", hint, select));
        match current.as_deref() {
            Some("chatgpt") if !self.more.models.is_empty() => {
                let chosen = self.voice_str("writerModel", cx).filter(|m| !m.is_empty()).unwrap_or_else(|| "gpt-6-luna".into());
                let mut models = vec!["gpt-6-luna".to_string()];
                models.extend(self.more.models.iter().filter(|m| *m != "gpt-6-luna").cloned());
                let model_select = self.select_button("llm-model", chosen, models.into_iter().map(|m| (m.clone(), m)).collect(), c, cx, |this, v, cx| this.set_rest("voice", "writerModel", json!(v), cx));
                llm_card = llm_card.child(row(c, "Model", None, model_select));
            }
            Some("claude") | Some("codex") => llm_card = llm_card.child(row(c, "Model", None, div().w(px(224.0)).child(Input::new(&writer_input)))),
            _ => {}
        }
        div().flex().flex_col().gap_6()
            .child(card(c, "ChatGPT account", Some("Runs the LLM below on your ChatGPT plan (Plus or Pro); no API key needed. Until you sign in, the Codex CLI's own login is used instead if Codex is installed. Set a weekly limit or disconnect winfer in ChatGPT's settings.")).child(account))
            .children(self.more.ai_result.as_ref().map(|r| result_text(c, r)))
            .child(llm_card)
    }
}

fn result_text(c: Colors, r: &Result<String, String>) -> Div {
    let (text, color) = match r {
        Ok(t) => (t.clone(), c.success),
        Err(t) => (t.clone(), c.danger),
    };
    div().px_1().text_xs().text_color(color).child(text)
}

#[cfg(test)]
mod tests {
    use super::live_connection;

    #[test]
    fn live_connection_follows_the_choice() {
        assert_eq!(live_connection("auto", true, true), Some("api"));
        assert_eq!(live_connection("auto", false, true), Some("chatgpt"));
        assert_eq!(live_connection("auto", false, false), None);
        assert_eq!(live_connection("chatgpt", true, false), Some("chatgpt"));
        assert_eq!(live_connection("apikey", false, false), Some("api"));
    }
}
