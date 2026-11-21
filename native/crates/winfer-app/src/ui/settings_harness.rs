//! Settings → winfer code and Mobile (port of `HarnessSettings` and `MobileSettings`).

use super::settings::{card, plain_button, row, stacked_row};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use winfer_backend::{harness, mobile};

static HINTS: LazyLock<HashMap<String, String>> = LazyLock::new(|| serde_json::from_str(include_str!("../../assets/settings-hints.json")).unwrap_or_default());

fn hint(key: &str) -> Option<String> {
    HINTS.get(key).cloned()
}

fn card_description(title: &str) -> String {
    HINTS.get(&format!("card:{title}")).cloned().unwrap_or_default()
}

const BACKENDS: [(&str, &str); 4] = [("chatgpt", "ChatGPT account"), ("claude", "Claude Code"), ("codex", "Codex"), ("local", "Local model")];
const WINFER_PROVIDER: &str = "winfer";
const MODES: [(&str, &str); 5] = [("ask", "Ask before edits and commands"), ("auto-edit", "Auto-edit (commands still ask)"), ("plan", "Plan (read-only)"), ("yolo-plan", "Yolo-plan (read-only, never ask)"), ("yolo", "Yolo (never ask)")];
const PERCENTS: [u64; 14] = [30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95];

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
    settings: Option<Value>,
    models: Option<Vec<Value>>,
    skills: Option<Vec<Value>>,
    commands: Option<Entity<InputState>>,
    mcp: Option<Entity<TextareaState>>,
    result: Option<Result<String, String>>,
    status: Option<Value>,
    pairing: Option<Value>,
    qr: Option<Arc<Image>>,
    relay: Option<Entity<InputState>>,
    mobile_result: Option<Result<String, String>>,
    polled: Option<std::time::Instant>,
    local: Option<LocalInputs>,
    local_test: Option<Result<String, String>>,
    local_testing: bool,
}

/// The text fields of Settings → winfer code → Local models. Edits stay in the fields until one
/// loses focus, so typing a host doesn't refetch models on every key.
struct LocalInputs {
    host: Entity<InputState>,
    port: Entity<InputState>,
    path: Entity<InputState>,
    key: Entity<InputState>,
    context: Entity<InputState>,
    efforts: Entity<InputState>,
    budgets: HashMap<String, Entity<InputState>>,
}

fn effort_label(level: &str) -> String {
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

fn digits(text: &str) -> u64 {
    text.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

/// Where the settings point, as the server's base URL (the hint under Host / Port).
fn local_base(local: &Value) -> String {
    let host = local["host"].as_str().unwrap_or_default().trim_end_matches('/');
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    let scheme = if host.contains("://") { "" } else { "http://" };
    let port = local["port"].as_u64().filter(|p| *p > 0).map_or(String::new(), |p| format!(":{p}"));
    let path = local["path"].as_str().unwrap_or_default().trim().trim_matches('/');
    format!("{scheme}{host}{port}{}", if path.is_empty() { String::new() } else { format!("/{path}") })
}

impl Root {
    fn load_harness(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.harness_page.settings.is_some() {
            return;
        }
        let settings = serde_json::to_value(harness::harness_settings()).unwrap_or_default();
        let commands_text = settings["allowedCommands"].as_array().map_or(String::new(), |a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
        let mcp_text = if settings["mcpServers"].as_object().is_some_and(|m| !m.is_empty()) { serde_json::to_string_pretty(&settings["mcpServers"]).unwrap_or_default() } else { String::new() };
        let commands = cx.new(|cx| InputState::new(window, cx).placeholder("git, cargo, pnpm").default_value(commands_text));
        let mcp = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 12).placeholder("{}").default_value(mcp_text));
        cx.subscribe(&commands, |this, input, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                let list = split_commands(&input.read(cx).value());
                this.save_harness("allowedCommands", json!(list), cx);
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
            let models = tauri::async_runtime::spawn(harness::harness_models()).await.ok().and_then(|m| serde_json::to_value(m).ok()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.harness_page.skills = Some(skills.as_array().cloned().unwrap_or_default());
                this.harness_page.models = Some(models.as_array().cloned().unwrap_or_default());
                cx.notify();
            });
        })
        .detach();
    }

    fn local_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn local_draft(&self, patch: Option<(&str, Value)>, cx: &App) -> Value {
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

    fn commit_local(&mut self, patch: Option<(&str, Value)>, window: Option<&mut Window>, cx: &mut Context<Self>) {
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

    fn test_local(&mut self, cx: &mut Context<Self>) {
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

    fn local_card(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.local_fields(window, cx);
        let enabled = self.harness_page.settings.as_ref().is_some_and(|s| s["local"]["enabled"] == true);
        let mut local = card(c, "Local models", Some("Run winfer code on a model served from your machine or network (vLLM, LM Studio, Ollama, llama.cpp, mtplx…), with winfer's own tools. The server needs the OpenAI Responses API (/v1/responses). Its models show up in the pickers as local/<model>."));
        let enabled_toggle = self.toggle("local-enabled", enabled, c, cx, move |this, cx| {
            this.commit_local(Some(("enabled", json!(!enabled))), None, cx);
        });
        local = local.child(row(c, "Use local models", None, enabled_toggle));
        if !enabled {
            return local;
        }
        let draft = self.local_draft(None, cx);
        let (efforts_now, flag): (Vec<String>, _) = (draft["efforts"].as_array().map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()), |key: &str| draft[key] == true);
        let (template, web) = (flag("templateEffort"), flag("webSearch"));
        for level in efforts_now.iter().filter(|e| *e != "none") {
            if !self.harness_page.local.as_ref().is_some_and(|i| i.budgets.contains_key(level)) {
                let value = draft["thinkingBudgets"][level].as_u64().map_or(String::new(), |n| n.to_string());
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("No cap").default_value(value));
                cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
                    if matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_local(None, Some(window), cx);
                    }
                })
                .detach();
                if let Some(i) = self.harness_page.local.as_mut() {
                    i.budgets.insert(level.clone(), input);
                }
            }
        }
        let inp = self.harness_page.local.as_ref().unwrap();
        let field = |e: &Entity<InputState>, w: f32| div().w(px(w)).child(Input::new(e));
        let host_port = div().flex().items_center().gap_1().child(field(&inp.host, 160.0)).child(div().text_color(c.subtle).child(":")).child(field(&inp.port, 80.0));
        let budgets: Vec<(String, Entity<InputState>)> = efforts_now.iter().filter(|e| *e != "none").filter_map(|l| inp.budgets.get(l).map(|i| (l.clone(), i.clone()))).collect();
        local = local
            .child(row(c, "Host / Port", Some(local_base(&draft)), host_port))
            .child(row(c, "API path", Some("Where the OpenAI-compatible API lives on the server.".into()), field(&inp.path, 112.0)))
            .child(row(c, "API key", Some("Sent as a bearer token. Most local servers don't need one.".into()), field(&inp.key, 192.0)))
            .child(row(c, "Context window", Some("Tokens, for the context meter and compaction. Automatic uses what the server reports for the model (else 32k).".into()), field(&inp.context, 112.0)))
            .child(row(c, "Thinking levels", Some("Comma-separated, as the model's chat template names them (Qwen3.8: low, medium, xhigh). none turns thinking off. Empty: only the model default. When the server shows the model's chat template (llama.cpp), its own levels are used instead, and a picked level is fitted to them (e.g. high → xhigh).".into()), field(&inp.efforts, 192.0)));
        let template_toggle = self.toggle("local-template", template, c, cx, move |this, cx| {
            this.commit_local(Some(("templateEffort", json!(!template))), None, cx);
        });
        local = local.child(row(c, "Send level to chat template", Some("Passes it as chat_template_kwargs.reasoning_effort, where llama.cpp, oMLX and MLX servers let the template read it. Turn off if your server rejects the field.".into()), template_toggle));
        for (level, input) in budgets {
            local = local.child(row(c, format!("Thinking budget · {}", effort_label(&level)), Some("Max thinking tokens at this level, sent as thinking_budget (oMLX enforces it). Empty: no cap. Too small a budget can cost accuracy.".into()), div().w(px(112.0)).child(Input::new(&input))));
        }
        let web_toggle = self.toggle("local-web", web, c, cx, move |this, cx| {
            this.commit_local(Some(("webSearch", json!(!web))), None, cx);
        });
        local = local.child(row(c, "Web search", Some("Gives the models a web_search tool when the server searches the web (oMLX: its Web search settings pick the provider and result count).".into()), web_toggle));
        let (testing, result) = (self.harness_page.local_testing, self.harness_page.local_test.clone());
        let hint = match &result {
            Some(Ok(t)) => div().text_xs().text_color(c.success).child(t.clone()),
            Some(Err(t)) => div().text_xs().text_color(c.danger).child(t.clone()),
            None => div().text_xs().text_color(c.subtle).child("Lists the server's models."),
        };
        let test = plain_button(c, "local-test", if testing { "Testing…" } else { "Test" }).on_click(cx.listener(move |this, _, _, cx| if !testing { this.test_local(cx) }));
        local.child(div().flex().items_center().gap_4().px_4().py_2p5()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().text_size(px(13.0)).text_color(c.fg).child("Connection")).child(hint))
            .child(test))
    }

    fn save_harness(&mut self, key: &str, value: Value, cx: &mut Context<Self>) {
        let Some(settings) = self.harness_page.settings.as_mut() else { return };
        settings[key] = value;
        let result = serde_json::from_value(settings.clone()).map_err(|e: serde_json::Error| e.to_string()).and_then(harness::harness_set_settings);
        self.harness_page.result = result.err().map(Err);
        cx.notify();
    }

    fn harness_toggle(&self, key: &'static str, on: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        self.toggle(SharedString::from(format!("harness-{key}")), on, c, cx, move |this, cx| this.save_harness(key, json!(!on), cx))
    }

    fn code_web(&self, key: &str, default: bool, cx: &App) -> bool {
        self.rest_bool("codeWeb", key, default, cx)
    }

    fn web_toggle(&self, key: &'static str, on: bool, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        self.toggle(SharedString::from(format!("web-{key}")), on, c, cx, move |this, cx| this.set_rest("codeWeb", key, json!(!on), cx))
    }

    pub(super) fn page_harness(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.load_harness(window, cx);
        let st = self.harness_page.settings.clone().unwrap();
        let models = self.harness_page.models.clone();
        let web = self.code_web("web", true, cx);
        let tui_toggle = self.toggle("web-tui", !web, c, cx, move |this, cx| this.set_rest("codeWeb", "web", json!(!web), cx));
        let mut defaults = card(c, "Defaults", Some("For winfer code, winfer's own coding agent (pick “winfer code” as an agent, or run winfer code in a terminal). New sessions start with these; /model, /effort, /mode and /compaction change them from inside a session too."))
            .child(row(c, "Use TUI style", hint("Use TUI style"), tui_toggle));
        if web {
            for (label, key, default) in [("Context meter", "showContext", true), ("Cost", "showCost", true), ("Tokens per second", "showRate", true), ("Plan above the composer", "showPlan", true), ("Show thinking", "expandThinking", false), ("Live shell output", "expandShell", false), ("Compact tool calls", "compactTools", true), ("Animated send button", "sendAnimation", true), ("Compact layout", "compact", false)] {
                let on = self.code_web(key, default, cx);
                let t = self.web_toggle(key, on, c, cx);
                defaults = defaults.child(row(c, label, hint(label), t));
            }
            let scale = self.state.read(cx).rest.get("codeWeb").and_then(|v| v["textScale"].as_f64()).unwrap_or(1.0);
            let step = move |by: f64| cx_scale(scale, by);
            let (down, up) = (step(-0.05), step(0.05));
            defaults = defaults.child(row(c, "Text size", None, div().flex().items_center().gap_2()
                .child(plain_button(c, "text-down", "A−").on_click(cx.listener(move |this, _, _, cx| this.set_rest("codeWeb", "textScale", json!(down), cx))))
                .child(div().w(px(48.0)).text_center().text_xs().text_color(c.muted).child(format!("{}%", (scale * 100.0).round())))
                .child(plain_button(c, "text-up", "A+").on_click(cx.listener(move |this, _, _, cx| this.set_rest("codeWeb", "textScale", json!(up), cx))))
                .child(plain_button(c, "text-reset", "Reset").on_click(cx.listener(|this, _, _, cx| this.set_rest("codeWeb", "textScale", json!(1.0), cx))))));
        }
        let model = st["model"].as_str().unwrap_or_default().to_string();
        let mut options = vec![(String::new(), "Automatic".to_string())];
        let list = models.clone().unwrap_or_default();
        for (id, label) in BACKENDS {
            options.extend(list.iter().filter(|m| m["spec"].as_str().is_some_and(|s| s.starts_with(&format!("{id}/")))).map(|m| (m["spec"].as_str().unwrap_or_default().to_string(), format!("{label} · {}", m["label"].as_str().unwrap_or_default()))));
        }
        if !model.is_empty() && !options.iter().any(|o| o.0 == model) {
            options.push((model.clone(), model.clone()));
        }
        let current = options.iter().find(|o| o.0 == model).map_or(model.clone(), |o| o.1.clone());
        let select = self.select_button("harness-model", current, options, c, cx, |this, v, cx| this.save_harness("model", json!(v), cx));
        defaults = defaults.child(row(c, "Model", hint("Model"), select));
        let effort = st["effort"].as_str().unwrap_or_default().to_string();
        let levels = |all: &[&str], empty: &str| std::iter::once((String::new(), empty.to_string())).chain(all.iter().map(|e| (e.to_string(), e.to_string()))).collect::<Vec<_>>();
        let effort_label = if effort.is_empty() { "Model default".to_string() } else { effort.clone() };
        let effort_select = self.select_button("harness-effort", effort_label, levels(&["minimal", "low", "medium", "high", "xhigh", "max"], "Model default"), c, cx, |this, v, cx| this.save_harness("effort", if v.is_empty() { Value::Null } else { json!(v) }, cx));
        defaults = defaults.child(row(c, "Reasoning effort", None, effort_select));
        for (id, label) in BACKENDS {
            let cur = st["subagents"][id].clone();
            let prefix = format!("{id}/");
            let mut opts = vec![(String::new(), "Same as main model".to_string())];
            opts.extend(list.iter().filter_map(|m| { let spec = m["spec"].as_str()?; (spec.starts_with(&prefix) && !spec.ends_with("-fast")).then(|| (spec[prefix.len()..].to_string(), m["label"].as_str().unwrap_or_default().to_string())) }));
            let current_model = cur["model"].as_str().unwrap_or_default().to_string();
            if !current_model.is_empty() && !opts.iter().any(|o| o.0 == current_model) {
                opts.push((current_model.clone(), current_model.clone()));
            }
            let shown = opts.iter().find(|o| o.0 == current_model).map_or(current_model.clone(), |o| o.1.clone());
            let effort_now = cur["effort"].clone();
            let keep = effort_now.clone();
            let model_select = self.select_button(&format!("sub-{id}"), shown, opts, c, cx, move |this, v, cx| {
                let Some(st) = this.harness_page.settings.as_ref() else { return };
                let mut next = st["subagents"].as_object().cloned().unwrap_or_default();
                if v.is_empty() { next.remove(id); } else { next.insert(id.to_string(), json!({ "model": v, "effort": keep })); }
                this.save_harness("subagents", Value::Object(next), cx);
            });
            let mut control = div().flex().flex_col().gap_1().child(model_select);
            if !current_model.is_empty() && id != "claude" {
                let (m, e) = (current_model.clone(), effort_now.as_str().unwrap_or_default().to_string());
                let e_label = if e.is_empty() { "Default effort".to_string() } else { e };
                control = control.child(self.select_button(&format!("sub-effort-{id}"), e_label, levels(&["minimal", "low", "medium", "high", "xhigh"], "Default effort"), c, cx, move |this, v, cx| {
                    let Some(st) = this.harness_page.settings.as_ref() else { return };
                    let mut next = st["subagents"].as_object().cloned().unwrap_or_default();
                    next.insert(id.to_string(), json!({ "model": m, "effort": if v.is_empty() { Value::Null } else { json!(v) } }));
                    this.save_harness("subagents", Value::Object(next), cx);
                }));
            }
            defaults = defaults.child(row(c, format!("Subagent model · {label}"), hint("Subagent model · ${b.label}"), control));
        }
        let mode = st["mode"].as_str().unwrap_or("ask").to_string();
        let mode_label = MODES.iter().find(|m| m.0 == mode).map_or(mode.clone(), |m| m.1.to_string());
        let mode_select = self.select_button("harness-mode", mode_label, MODES.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(), c, cx, |this, v, cx| this.save_harness("mode", json!(v), cx));
        let approve_on = self.state.read(cx).auto_approve.iter().any(|p| p == WINFER_PROVIDER);
        let approve = self.toggle("harness-approve", approve_on, c, cx, move |this, cx| this.update_state(cx, |s| {
            if approve_on { s.auto_approve.retain(|p| p != WINFER_PROVIDER); } else { s.auto_approve.push(WINFER_PROVIDER.into()); }
        }));
        let sidebar_toggle = self.harness_toggle("planSidebar", st["planSidebar"] == true, c, cx);
        defaults = defaults
            .child(row(c, "Permission mode", Some(if mode == "yolo" { "Nothing asks before editing files or running commands.".into() } else { "Shift+Tab cycles it inside a session.".into() }), mode_select))
            .child(row(c, "Auto-approve in panes", Some(if approve_on { "winfer code panes start in yolo mode (--yolo), overriding the permission mode above. Applies to panes started or restarted after the change.".into() } else { "Start winfer code panes in yolo mode: nothing asks before editing files or running commands.".into() }), approve))
            .child(row(c, "Plan sidebar", hint("Plan sidebar"), sidebar_toggle));
        let (bridges, lean_codex, prompt) = (st["leanBridges"] == true, st["leanCodex"] == true, st["leanCodexPrompt"] == true);
        let pane_lean = self.state.read(cx).lean_codex;
        let t1 = self.harness_toggle("leanBridges", bridges, c, cx);
        let t2 = self.harness_toggle("leanCodex", lean_codex, c, cx);
        let t3 = self.toggle("lean-panes", pane_lean, c, cx, |this, cx| this.update_state(cx, |s| s.lean_codex = !s.lean_codex));
        let mut lean = card(c, "Lean setup", Some(&card_description("Lean setup"))).child(row(c, "Claude Code", hint("Claude Code"), t1)).child(row(c, "Codex", hint("Codex"), t2));
        if lean_codex {
            let t = self.harness_toggle("leanCodexPrompt", prompt, c, cx);
            lean = lean.child(row(c, "Short Codex prompt", hint("Short Codex prompt"), t));
        }
        lean = lean.child(row(c, "Codex CLI panes too", hint("Codex CLI panes too"), t3));
        if bridges || lean_codex || pane_lean {
            let all = st["leanAllSkills"] == true;
            let select = self.select_button("lean-skills", if all { "All skills".into() } else { "Selected only".into() }, vec![("all".into(), "All skills".into()), ("selected".into(), "Selected only".into())], c, cx, |this, v, cx| this.save_harness("leanAllSkills", json!(v == "all"), cx));
            lean = lean.child(row(c, "Skills", hint(if all { "skillsAll" } else { "skillsSel" }), select));
            if !all {
                let picked: Vec<String> = st["leanSkills"].as_array().map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
                let skills = self.harness_page.skills.clone();
                let count = picked.iter().filter(|id| skills.iter().flatten().any(|s| s["id"] == id.as_str())).count();
                let body: AnyElement = match &skills {
                    None => div().text_xs().text_color(c.subtle).child("Loading…").into_any_element(),
                    Some(l) if l.is_empty() => div().text_xs().text_color(c.subtle).child("No skills found.").into_any_element(),
                    Some(l) => {
                        let mut chips = div().flex().flex_wrap().gap_1p5();
                        for s in l {
                            let id = s["id"].as_str().unwrap_or_default().to_string();
                            let on = picked.contains(&id);
                            let source = s["source"].as_str().unwrap_or_default();
                            let name = s["name"].as_str().unwrap_or_default();
                            let label = if source == "user" { name.to_string() } else { format!("{source}:{name}") };
                            let current = picked.clone();
                            chips = chips.child(div().id(SharedString::from(format!("lean-{id}"))).flex().h(px(24.0)).items_center().rounded_md().border_1().px_2().text_size(px(11.0)).cursor_pointer()
                                .when(on, |d| d.text_color(c.fg)).when(!on, |d| d.text_color(c.muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let mut next = current.clone();
                                    if let Some(i) = next.iter().position(|x| *x == id) { next.remove(i); } else { next.push(id.clone()); }
                                    this.save_harness("leanSkills", json!(next), cx);
                                }))
                                .child(label));
                        }
                        chips.into_any_element()
                    }
                };
                lean = lean.child(stacked_row(c, format!("Selected skills ({count})"), hint("Selected skills (${settings.leanSkills.filter((id) => claudeSkills?.some((s) => s.id === id)).length})"), body));
            }
        }
        let percent = |this: &Self, key: &'static str, value: u64, cx: &mut Context<Self>| {
            let mut options = vec![("0".to_string(), "Off".to_string())];
            options.extend(PERCENTS.iter().map(|p| (p.to_string(), format!("{p}%"))));
            this.select_button(key, if value == 0 { "Off".into() } else { format!("{value}%") }, options, c, cx, move |this, v, cx| this.save_harness(key, json!(v.parse::<u64>().unwrap_or(0)), cx))
        };
        let (trim, compact) = (st["trimAt"].as_u64().unwrap_or(0), st["compactAt"].as_u64().unwrap_or(0));
        let overlap = trim > 0 && compact > 0 && trim >= compact;
        let trim_select = percent(self, "trimAt", trim, cx);
        let compact_select = percent(self, "compactAt", compact, cx);
        let compaction = card(c, "Context compaction", Some(&card_description("Context compaction")))
            .child(row(c, "Micro-compact at", hint("Micro-compact at"), trim_select))
            .child(row(c, "Full compact at", hint(if overlap { "overlap" } else { "compactHint" }), compact_select));
        let i = (self.harness_page.commands.clone().unwrap(), self.harness_page.mcp.clone().unwrap());
        let tools = card(c, "Tools", Some(&card_description("Tools")))
            .child(row(c, "Always-allowed commands", hint("Always-allowed commands"), div().w(px(224.0)).child(Input::new(&i.0))))
            .child(stacked_row(c, "MCP servers", Some("Extra servers for every backend, as JSON: { \"name\": { \"command\": \"…\", \"args\": [], \"env\": {} } }. winfer's own server is added automatically in panes.".to_string()), Textarea::new(&i.1)));
        let local = self.local_card(c, window, cx);
        div().flex().flex_col().gap_6().child(defaults).child(local).child(lean).child(compaction).child(tools).children(self.harness_page.result.as_ref().and_then(|r| r.as_ref().err()).map(|e| div().px_1().text_xs().text_color(c.danger).child(e.clone())))
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
        let pairing = self.harness_page.pairing.clone();
        let pending = pairing.as_ref().and_then(|p| status.as_ref()?["devices"].as_array()?.iter().find(|d| d["deviceId"] == p["deviceId"]).cloned());
        let pair = plain_button(c, "mobile-pair", if pairing.is_some() { "New code" } else { "Pair phone" }).on_click(cx.listener(|this, _, _, cx| {
            this.harness_page.mobile_result = None;
            match mobile::mobile_pair_start().map(|p| serde_json::to_value(p).unwrap_or_default()) {
                Ok(p) => {
                    this.harness_page.qr = p["qrSvg"].as_str().map(|svg| Arc::new(Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())));
                    this.harness_page.pairing = Some(p);
                }
                Err(e) => this.harness_page.mobile_result = Some(Err(e)),
            }
            this.refresh_mobile(cx);
        }));
        let mut phone = card(c, "Phone", Some(&card_description("Phone"))).child(row(c, "Pair a phone", hint("Pair a phone"), pair));
        if let Some(p) = pairing {
            let link = p["link"].as_str().unwrap_or_default().to_string();
            let waiting = match &pending {
                Some(d) if d["connected"] == true => "Waiting for your phone…".to_string(),
                Some(d) if d["error"].is_string() => format!("Can't reach the relay: {}", d["error"].as_str().unwrap_or_default()),
                _ => "Connecting to the relay…".to_string(),
            };
            phone = phone.child(div().flex().items_center().gap_4().px_3p5().py_3()
                .child(div().size(px(180.0)).flex_shrink_0().overflow_hidden().rounded_md().bg(gpui_kit::white()).p_1().children(self.harness_page.qr.clone().map(|q| img(q).size_full())))
                .child(div().min_w_0().flex().flex_col().gap_2().text_xs().text_color(c.muted).child(waiting).child("The code works once, for 10 minutes. It holds the key to your agents: don't share it.")
                    .child(div().child(plain_button(c, "mobile-copy", "Copy link").on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                        this.harness_page.mobile_result = Some(Ok("Link copied.".into()));
                        cx.notify();
                    }))))));
        }
        let now = now_ms();
        for d in status.iter().flat_map(|s| s["devices"].as_array().into_iter().flatten()).filter(|d| d["paired"] == true) {
            let phones = d["phones"].as_u64().unwrap_or(0);
            let connected = d["connected"] == true;
            let mut parts = vec![if phones > 0 { "Open now".to_string() } else { d["lastSeen"].as_f64().map_or("Paired".to_string(), |t| format!("Last seen {}", ago(t, now))) }, if d["push"] == true { "notifications on".into() } else { "notifications off".into() }];
            if !connected && let Some(e) = d["error"].as_str() {
                parts.push(format!("relay: {e}"));
            }
            let (id, name) = (d["deviceId"].as_str().unwrap_or_default().to_string(), d["name"].as_str().unwrap_or_default().to_string());
            let button_name = name.clone();
            phone = phone.child(row(c, name, Some(parts.join(" · ")), div().flex().items_center().gap_2()
                .child(div().size_2().rounded_full().bg(if phones > 0 { c.success } else if connected { c.subtle } else { c.danger }))
                .child(plain_button(c, SharedString::from(format!("unpair-{id}")), "Unpair").on_click(cx.listener(move |this, _, window, cx| {
                    let target = id.clone();
                    this.confirm(&format!("Unpair {button_name}"), format!("{button_name} will stop getting notifications and can't reach your agents until you pair it again."), "Unpair", window, cx, move |this, cx| {
                        if let Err(e) = mobile::mobile_revoke(target.clone()) {
                            this.harness_page.mobile_result = Some(Err(e));
                        }
                        this.refresh_mobile(cx);
                    });
                })))));
        }
        let url = status.as_ref().and_then(|s| s["relayUrl"].as_str()).unwrap_or_default().to_string();
        let default = status.as_ref().and_then(|s| s["defaultRelayUrl"].as_str()).unwrap_or_default().to_string();
        let can_save = status.is_some() && relay.read(cx).value().trim().trim_end_matches('/') != url;
        let save_from = relay.clone();
        let relay_card = card(c, "Relay", Some(&card_description("Relay"))).child(row(c, "Relay URL", (url != default).then(|| format!("Default: {default}")), div().flex().items_center().gap_2()
            .child(div().w(px(256.0)).child(Input::new(&relay)))
            .child(plain_button(c, "relay-save", "Save").when(!can_save, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, _, cx| {
                if !can_save {
                    return;
                }
                let value = save_from.read(cx).value().to_string();
                this.harness_page.mobile_result = Some(mobile::mobile_set_relay_url(value).map(|_| "Relay saved. Phones reconnect to it.".to_string()));
                this.refresh_mobile(cx);
            })))));
        div().flex().flex_col().gap_6().child(phone).child(relay_card).children(self.harness_page.mobile_result.as_ref().map(|r| {
            let (t, col) = match r { Ok(t) => (t.clone(), c.success), Err(t) => (t.clone(), c.danger) };
            div().px_1().text_xs().text_color(col).child(t)
        }))
    }
}

fn cx_scale(scale: f64, by: f64) -> f64 {
    crate::ui::code_view::text_scale_step(scale, by)
}

#[cfg(test)]
mod tests {
    use super::{ago, cx_scale, digits, effort_label, local_base, parse_efforts, parse_servers, split_commands};
    use serde_json::json;

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
        assert_eq!(cx_scale(1.0, 0.05), 1.05);
        assert_eq!(cx_scale(1.5, 0.05), 1.5);
    }

    #[test]
    fn local_model_fields() {
        assert_eq!(parse_efforts("None, low  Medium,low,, xhigh"), ["none", "low", "medium", "xhigh"]);
        assert!(parse_efforts(" , ").is_empty());
        assert_eq!(digits("8 0a0"), 800);
        assert_eq!(effort_label("xhigh"), "Extra high");
        assert_eq!(effort_label("low"), "Low");
        let l = |host: &str, port: u64, path: &str| local_base(&json!({ "host": host, "port": port, "path": path }));
        assert_eq!(l("127.0.0.1", 8000, "/v1"), "http://127.0.0.1:8000/v1");
        assert_eq!(l("", 8000, "v1/"), "http://127.0.0.1:8000/v1");
        assert_eq!(l("https://box.lan/", 0, "/v1"), "https://box.lan/v1");
        assert_eq!(l("localhost", 11434, ""), "http://localhost:11434");
    }
}
