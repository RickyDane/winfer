//! OpenCode bridge: `opencode acp`, OpenCode's Agent Client Protocol server (JSON-RPC over
//! stdio). One process holds the session: prompts stream back as `session/update`s, permission
//! prompts come as `session/request_permission` requests answered by the TUI (or by the bridge
//! itself, per mode), and `session/cancel` interrupts. Model, effort and the plan agent are the
//! session's config options.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Skill, Todo, TurnTokens, Usage, UserMsg};
use crate::harness::tools::short_path;

/// The models `opencode` offers, as `opencode/<provider>/<model>` specs; empty when it isn't installed.
pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("opencode").is_none() {
        return Vec::new();
    }
    let (tx, rx) = channel();
    let Ok(_proc) = LineProcess::spawn("opencode", &["models".to_string()], &[], &std::env::temp_dir(), tx) else {
        return Vec::new();
    };
    let mut out = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(In::Line(line)) => {
                out.push_str(&line);
                out.push('\n');
            }
            Ok(In::Closed) => break,
            _ => return Vec::new(),
        }
    }
    crate::providers::parse_models(&out)
        .into_iter()
        .filter_map(|m| {
            let (provider, model) = m.id.split_once('/')?;
            Some(ModelInfo { spec: format!("opencode/{}", m.id), label: label(model), detail: format!("OpenCode · {provider}"), efforts: efforts(provider, model) })
        })
        .collect()
}

/// OpenCode's default effort variants per provider (`packages/core/src/variant.ts`); models.dev
/// can narrow them, so they are a best guess. Minimal isn't listed: it goes to OpenCode as low.
fn efforts(provider: &str, model: &str) -> Option<Vec<String>> {
    let list: &[&str] = match provider {
        "anthropic" if model.contains("4-6") || model.contains("4.6") => &["low", "medium", "high", "max"],
        "anthropic" if ["fable", "mythos"].iter().any(|f| model.contains(f)) => &["low", "medium", "high", "xhigh", "max"],
        "anthropic" => &["none", "low", "medium", "high", "xhigh", "max"],
        "openai" => &["none", "low", "medium", "high", "xhigh"],
        "google" => &["low", "medium", "high"],
        _ => return None,
    };
    Some(list.iter().map(|s| s.to_string()).collect())
}

/// A readable name for a model id: `gpt-5.6-luna-fast` → "GPT-5.6 Luna Fast".
fn label(id: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for word in id.split(['-', '_']).filter(|w| !w.is_empty()) {
        let numeric = word.starts_with(|c: char| c.is_ascii_digit());
        let acronym = ["gpt", "glm", "vl", "ai", "oss", "llm"].contains(&word);
        let w = if acronym || (numeric && word.len() <= 5) {
            word.to_ascii_uppercase()
        } else {
            let mut c = word.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        };
        // "GPT-5.6", not "GPT 5.6".
        match words.last_mut() {
            Some(last) if numeric && last.chars().all(|c| c.is_ascii_uppercase()) && last.len() <= 3 => {
                last.push('-');
                last.push_str(&w);
            }
            _ => words.push(w),
        }
    }
    words.join(" ")
}

/// Every edit, command and fetch asks, so the bridge can decide per mode (and follow mode
/// changes mid-session); OpenCode's defaults allow them all. Added to the user's own
/// `OPENCODE_CONFIG_CONTENT`, if set.
fn config_env(existing: Option<&str>) -> String {
    let mut config = existing
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if !config["permission"].is_object() {
        config["permission"] = json!({});
    }
    for tool in ["edit", "bash", "webfetch"] {
        config["permission"][tool] = json!("ask");
    }
    config.to_string()
}

/// winfer's MCP servers (`name → { command, args, env }`, or `{ type: "http", url }`) as ACP's `mcpServers`.
fn mcp_servers(servers: &serde_json::Map<String, Value>) -> Value {
    let pairs = |v: &Value| -> Vec<Value> {
        v.as_object().into_iter().flatten().map(|(k, v)| json!({ "name": k, "value": v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()) })).collect()
    };
    servers
        .iter()
        .map(|(name, s)| match s["url"].as_str() {
            Some(url) => json!({ "type": "http", "name": name, "url": url, "headers": pairs(&s["headers"]) }),
            None => json!({
                "name": name,
                "command": s["command"].as_str().unwrap_or(""),
                "args": s["args"].as_array().cloned().unwrap_or_default(),
                "env": pairs(&s["env"]),
            }),
        })
        .collect()
}

/// What a request of ours was for, by its JSON-RPC id.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pending {
    Init,
    Session,
    /// Switching the model; the effort is set after it (its levels are the model's).
    Model,
    Config,
    Effort,
    Prompt,
}

#[derive(Default)]
struct Call {
    name: String,
    kind: String,
    input: Value,
    started: bool,
    /// Output text already shown.
    shown: usize,
    /// The file before a `write`, for its diff.
    before: Option<String>,
    /// The diff its permission prompt showed.
    diff: Option<String>,
}

/// A permission prompt waiting on the user: OpenCode's request id and its options.
struct Ask {
    rpc: Value,
    options: Vec<Value>,
}

struct Bridge {
    events: Events,
    /// Lines for OpenCode, written by `run`.
    outbox: Vec<Value>,
    seq: u64,
    pending: HashMap<u64, Pending>,
    cwd: String,
    mcp: Value,
    resume: Option<String>,
    session: Option<String>,
    mode: Mode,
    bg_owner: String,
    /// The model and effort asked for (`provider/model`, OpenCode's effort level).
    model: String,
    effort: Option<String>,
    /// The session's config options (model, effort, mode), as OpenCode last sent them.
    config: Value,
    shown_model: String,
    /// A turn is open (TurnStart sent, TurnDone not yet).
    busy: bool,
    /// The running prompt's request id.
    prompt: Option<u64>,
    queue: VecDeque<UserMsg>,
    /// Goes before the first message (ACP has no system prompt).
    prefix: Option<String>,
    allowed: Vec<String>,
    calls: HashMap<String, Call>,
    asks: HashMap<String, Ask>,
    usage: Usage,
    turn_tokens: TurnTokens,
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let config = config_env(std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref());
    let mut proc = match LineProcess::spawn("opencode", &["acp".to_string()], &[("OPENCODE_CONFIG_CONTENT", &config)], &opts.cwd, me) {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(Ev::Exited(e));
            return;
        }
    };
    let mut b = Bridge::new(&opts, events);
    crate::harness::bg::set_mode(&b.bg_owner, b.mode);
    b.request("initialize", json!({ "protocolVersion": 1, "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false } }), Pending::Init);
    loop {
        for out in std::mem::take(&mut b.outbox) {
            if let Err(e) = proc.write(&out) {
                b.emit(Ev::Error(format!("couldn't reach OpenCode: {e}")));
            }
        }
        match super::recv(&rx) {
            None => return,
            Some(In::Line(line)) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    b.on_message(&v);
                }
            }
            Some(In::Closed) => {
                let _ = b.events.send(Ev::Exited(format!("OpenCode stopped: {}", proc.exit_reason())));
                return;
            }
            Some(In::Cmd(cmd)) => {
                if !b.on_cmd(cmd) {
                    return;
                }
            }
        }
    }
}

impl Bridge {
    fn new(opts: &Opts, events: Events) -> Self {
        // `provider/model#variant` picks the variant as the effort.
        let (model, variant) = match opts.model.split_once('#') {
            Some((m, v)) => (m.to_string(), Some(v.to_string())),
            None => (opts.model.clone(), None),
        };
        Bridge {
            events,
            outbox: Vec::new(),
            seq: 0,
            pending: HashMap::new(),
            cwd: opts.cwd.to_string_lossy().into_owned(),
            mcp: mcp_servers(&opts.mcp),
            resume: opts.resume.clone(),
            session: None,
            mode: opts.mode,
            bg_owner: opts.bg_owner.clone(),
            model,
            effort: opts.effort.clone().or(variant),
            config: Value::Null,
            shown_model: String::new(),
            busy: false,
            prompt: None,
            queue: VecDeque::new(),
            prefix: opts.system.clone().filter(|_| opts.resume.is_none()),
            allowed: opts.allowed_commands.clone(),
            calls: HashMap::new(),
            asks: HashMap::new(),
            usage: Usage::default(),
            turn_tokens: TurnTokens::default(),
        }
    }

    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn request(&mut self, method: &str, params: Value, kind: Pending) -> u64 {
        self.seq += 1;
        self.pending.insert(self.seq, kind);
        self.outbox.push(json!({ "jsonrpc": "2.0", "id": self.seq, "method": method, "params": params }));
        self.seq
    }

    fn respond(&mut self, id: Value, result: Value) {
        self.outbox.push(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn set_option(&mut self, id: &str, value: &str, kind: Pending) {
        let Some(session) = self.session.clone() else { return };
        self.request("session/set_config_option", json!({ "sessionId": session, "configId": id, "value": value }), kind);
    }

    /// The session's value of a config option (`model`, `effort`, `mode`).
    fn option(&self, id: &str) -> Option<&Value> {
        self.config.as_array()?.iter().find(|o| o["id"] == id)
    }

    fn agent(mode: Mode) -> &'static str {
        if matches!(mode, Mode::Plan | Mode::YoloPlan) { "plan" } else { "build" }
    }

    /// Ready for prompts: the session is there and its settings applied.
    fn ready(&self) -> bool {
        self.session.is_some() && !self.pending.values().any(|p| matches!(p, Pending::Session | Pending::Model | Pending::Config | Pending::Effort))
    }

    fn set_effort(&mut self) {
        let Some(option) = self.option("effort") else {
            if self.effort.is_some() {
                self.emit(Ev::Notice("This model has no effort levels in OpenCode.".into()));
            }
            return;
        };
        let level = match self.effort.as_deref() {
            None | Some("") => "default",
            Some("minimal") => "low",
            Some(e) => e,
        }
        .to_string();
        let offered = option["options"].as_array().into_iter().flatten().any(|o| o["value"] == level.as_str());
        if offered {
            self.set_option("effort", &level, Pending::Effort);
        } else {
            self.emit(Ev::Notice(format!("Effort `{level}` isn't available for this model in OpenCode.")));
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Send(msg) => self.send(msg),
            Cmd::Steer(msg) if self.busy => {
                self.emit(Ev::Notice("OpenCode takes this after the current turn.".into()));
                self.queue.push_back(msg);
            }
            Cmd::Steer(msg) => self.send(msg),
            Cmd::Interrupt => {
                self.queue.clear();
                for (_, ask) in std::mem::take(&mut self.asks) {
                    self.respond(ask.rpc, json!({ "outcome": { "outcome": "cancelled" } }));
                }
                if let (Some(session), Some(_)) = (self.session.clone(), self.prompt) {
                    self.outbox.push(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
                } else if self.busy {
                    // Not sent yet (the session is still starting).
                    self.busy = false;
                    self.emit(Ev::TurnDone);
                }
            }
            Cmd::Approve(id, decision) => {
                let kind = match decision {
                    Decision::Allow => "allow_once",
                    Decision::Always => "allow_always",
                    Decision::Deny => "reject_once",
                };
                self.answer(&id, kind);
            }
            Cmd::Answer(id, _) => self.answer(&id, "allow_once"),
            Cmd::SetModel(model) => {
                let (model, variant) = match model.split_once('#') {
                    Some((m, v)) => (m.to_string(), Some(v.to_string())),
                    None => (model, None),
                };
                self.model = model.clone();
                if variant.is_some() {
                    self.effort = variant;
                }
                if !model.is_empty() {
                    self.set_option("model", &model, Pending::Model);
                }
            }
            Cmd::SetEffort(e) => {
                self.effort = e;
                self.set_effort();
            }
            Cmd::SetMode(mode) => {
                let was = Self::agent(self.mode);
                self.mode = mode;
                crate::harness::bg::set_mode(&self.bg_owner, mode);
                if Self::agent(mode) != was {
                    self.set_option("mode", Self::agent(mode), Pending::Config);
                }
            }
            Cmd::Compact => self.send(UserMsg { text: "/compact".into(), images: Vec::new(), skill: None }),
            // OpenCode compacts the session on its own when it fills up.
            Cmd::SetCompaction { .. } => {}
            // Its subagents (if any) run on the main model.
            Cmd::SetSubagent { .. } => {}
            Cmd::Undo { .. } => self.emit(Ev::Undone(Err("OpenCode can't undo a turn from winfer code.".into()))),
            Cmd::StopTask(id) if crate::harness::bg::stop(&id) => {}
            Cmd::StopTask(_) => self.emit(Ev::Notice("OpenCode's subagents can't be stopped on their own; Esc stops the turn.".into())),
            Cmd::Shutdown => return false,
        }
        true
    }

    fn send(&mut self, msg: UserMsg) {
        if self.busy {
            self.queue.push_back(msg);
            return;
        }
        self.busy = true;
        self.emit(Ev::TurnStart);
        self.queue.push_front(msg);
        self.flush();
    }

    /// Sends the next queued message once the session is ready and no prompt is running.
    fn flush(&mut self) {
        if self.prompt.is_some() || !self.ready() {
            return;
        }
        let Some(msg) = self.queue.pop_front() else { return };
        if !self.busy {
            self.busy = true;
            self.emit(Ev::TurnStart);
        }
        let mut text = match &msg.skill {
            Some(skill) => format!("/{skill} {}", msg.text).trim_end().to_string(),
            None => msg.text.clone(),
        };
        // Commands go as they are (the system prompt waits for the first real message).
        if !text.starts_with('/') {
            if let Some(prefix) = self.prefix.take() {
                text = format!("{prefix}\n\n{text}");
            }
        }
        let mut prompt = vec![json!({ "type": "text", "text": text })];
        for path in &msg.images {
            if let Some((media, data)) = crate::harness::tools::image_base64(path) {
                prompt.push(json!({ "type": "image", "mimeType": media, "data": data }));
            }
        }
        self.turn_tokens = TurnTokens::default();
        let session = self.session.clone().unwrap_or_default();
        self.prompt = Some(self.request("session/prompt", json!({ "sessionId": session, "prompt": prompt }), Pending::Prompt));
    }

    /// Answers a permission prompt with the option of that kind.
    fn answer(&mut self, id: &str, kind: &str) {
        let Some(ask) = self.asks.remove(id) else { return };
        let option = ask
            .options
            .iter()
            .find(|o| o["kind"] == kind)
            .or_else(|| ask.options.iter().find(|o| o["kind"].as_str().is_some_and(|k| k.starts_with(&kind[..kind.find('_').unwrap_or(0)]))))
            .and_then(|o| o["optionId"].as_str());
        let outcome = match option {
            Some(option) => json!({ "outcome": { "outcome": "selected", "optionId": option } }),
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        };
        self.respond(ask.rpc, outcome);
    }

    fn on_message(&mut self, v: &Value) {
        match (v.get("method").and_then(Value::as_str), v.get("id")) {
            (Some(method), Some(id)) => self.on_request(method, id.clone(), &v["params"]),
            (Some("session/update"), None) => self.on_update(&v["params"]),
            (Some(_), None) => {}
            (None, Some(id)) => {
                let Some(kind) = id.as_u64().and_then(|id| self.pending.remove(&id)) else { return };
                self.on_response(kind, v);
                self.flush();
            }
            (None, None) => {}
        }
    }

    fn on_response(&mut self, kind: Pending, v: &Value) {
        let error = v.get("error").map(|e| {
            let message = e["message"].as_str().unwrap_or("request failed");
            match e["data"].as_str().or_else(|| e["data"]["message"].as_str()) {
                Some(data) if !data.is_empty() && data != message => format!("{message}: {data}"),
                _ => message.to_string(),
            }
        });
        let result = &v["result"];
        match kind {
            Pending::Init => {
                if let Some(e) = error {
                    self.emit(Ev::Exited(format!("OpenCode didn't start: {e}")));
                    return;
                }
                let params = json!({ "sessionId": self.resume, "cwd": self.cwd, "mcpServers": self.mcp });
                match self.resume.clone() {
                    Some(_) => self.request("session/resume", params, Pending::Session),
                    None => self.request("session/new", json!({ "cwd": self.cwd, "mcpServers": self.mcp }), Pending::Session),
                };
            }
            Pending::Session => {
                if let Some(e) = error {
                    if self.resume.take().is_some() {
                        // The session is gone: start a new one.
                        self.emit(Ev::Notice(format!("Couldn't continue the OpenCode session ({e}); starting a new one.")));
                        self.request("session/new", json!({ "cwd": self.cwd, "mcpServers": self.mcp }), Pending::Session);
                    } else {
                        self.emit(Ev::Exited(format!("OpenCode couldn't start a session: {e}")));
                    }
                    return;
                }
                let id = result["sessionId"].as_str().map(str::to_string).or_else(|| self.resume.clone()).unwrap_or_default();
                self.session = Some(id.clone());
                self.emit(Ev::Session(id));
                self.config = result["configOptions"].clone();
                let current = self.option("model").and_then(|o| o["currentValue"].as_str()).unwrap_or("").to_string();
                if !self.model.is_empty() && self.model != current {
                    let model = self.model.clone();
                    self.set_option("model", &model, Pending::Model);
                } else if self.effort.is_some() {
                    self.set_effort();
                }
                let agent = Self::agent(self.mode);
                if self.option("mode").and_then(|o| o["currentValue"].as_str()).is_some_and(|m| m != agent) {
                    self.set_option("mode", agent, Pending::Config);
                }
                if !self.pending.values().any(|p| *p == Pending::Model) {
                    self.show_model();
                }
            }
            Pending::Model | Pending::Config | Pending::Effort => {
                match error {
                    Some(e) if kind == Pending::Effort => self.emit(Ev::Notice(format!("OpenCode didn't take the effort: {e}"))),
                    Some(e) => self.emit(Ev::Error(format!("OpenCode: {e}"))),
                    None => {
                        if result["configOptions"].is_array() {
                            self.config = result["configOptions"].clone();
                        }
                    }
                }
                if kind == Pending::Model && self.effort.is_some() {
                    self.set_effort();
                }
                self.show_model();
            }
            Pending::Prompt => {
                self.prompt = None;
                if let Some(e) = error {
                    self.emit(Ev::Error(e));
                }
                let u = &result["usage"];
                if u.is_object() {
                    let n = |k: &str| u[k].as_u64().unwrap_or(0);
                    let input = n("inputTokens") + n("cachedReadTokens") + n("cachedWriteTokens");
                    self.usage.input += input;
                    self.usage.cached += n("cachedReadTokens");
                    self.usage.output += n("outputTokens");
                    self.usage.turn_input = input;
                    self.usage.turn_cached = n("cachedReadTokens");
                    self.turn_tokens.exact(n("outputTokens"));
                    self.emit_usage();
                }
                match result["stopReason"].as_str() {
                    Some("max_tokens") => self.emit(Ev::Notice("The model hit its output limit.".into())),
                    Some("max_turn_requests") => self.emit(Ev::Notice("OpenCode stopped: too many steps in one turn.".into())),
                    Some("refusal") => self.emit(Ev::Notice("The model declined to continue.".into())),
                    _ => {}
                }
                // Prompts it never answered end with the turn.
                for (_, ask) in std::mem::take(&mut self.asks) {
                    self.respond(ask.rpc, json!({ "outcome": { "outcome": "cancelled" } }));
                }
                self.busy = false;
                self.emit(Ev::TurnDone);
            }
        }
    }

    fn show_model(&mut self) {
        let current = self.option("model").and_then(|o| o["currentValue"].as_str()).unwrap_or("").to_string();
        if !current.is_empty() && current != self.shown_model {
            self.shown_model = current.clone();
            self.emit(Ev::Model(current));
        }
    }

    fn emit_usage(&self) {
        let mut u = self.usage.clone();
        u.turn = self.turn_tokens.now();
        u.turn_streamed = self.turn_tokens.streamed_total();
        self.emit(Ev::Usage(u));
    }

    fn on_request(&mut self, method: &str, id: Value, p: &Value) {
        if method != "session/request_permission" {
            self.outbox.push(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("winfer code doesn't support {method}") } }));
            return;
        }
        let tool = &p["toolCall"];
        let call_id = tool["toolCallId"].as_str().unwrap_or("").to_string();
        self.track(tool, true);
        let call = self.calls.get(&call_id);
        let name = call.map(|c| c.name.clone()).unwrap_or_default();
        let kind = call.map(|c| c.kind.clone()).unwrap_or_default();
        let input = call.map(|c| c.input.clone()).unwrap_or(Value::Null);
        let key = format!("p{}", id.as_u64().map(|n| n.to_string()).unwrap_or_else(|| id.to_string()));
        self.asks.insert(key.clone(), Ask { rpc: id, options: p["options"].as_array().cloned().unwrap_or_default() });
        let command = input["command"].as_str().unwrap_or("");
        let allowed = !command.is_empty() && self.allowed.iter().any(|c| command == c || command.starts_with(&format!("{c} ")));
        let auto = match self.mode {
            Mode::Yolo | Mode::YoloPlan => true,
            Mode::AutoEdit => kind == "edit",
            Mode::Ask | Mode::Plan => false,
        };
        if auto || allowed {
            self.answer(&key, "allow_once");
            return;
        }
        let diff = content_diff(&tool["content"]);
        if let Some(c) = self.calls.get_mut(&call_id) {
            c.diff = diff.clone();
        }
        let title = title(&name, &input);
        let title = if title.is_empty() { tool["title"].as_str().unwrap_or(&name).to_string() } else { title };
        let detail = match command {
            "" => input["path"].as_str().or(input["url"].as_str()).unwrap_or("").to_string(),
            c => c.to_string(),
        };
        self.emit(Ev::Approval(Approval { id: key, tool: name, title, detail, diff, questions: Vec::new() }));
    }

    /// Records what a `tool_call` / `tool_call_update` tells about a call, and shows its card once
    /// its input is complete: when it runs or asks (a pending call's input is still partial).
    fn track(&mut self, t: &Value, asking: bool) {
        let Some(id) = t["toolCallId"].as_str() else { return };
        let call = self.calls.entry(id.to_string()).or_default();
        if let Some(name) = t["name"].as_str() {
            call.name = name.to_string();
        } else if call.name.is_empty() {
            call.name = t["title"].as_str().unwrap_or("tool").to_string();
        }
        if let Some(kind) = t["kind"].as_str() {
            call.kind = kind.to_string();
        }
        if t["rawInput"].as_object().is_some_and(|o| !o.is_empty()) {
            call.input = t["rawInput"].clone();
        }
        let running = !matches!(t["status"].as_str(), None | Some("pending"));
        if !call.started && (asking || running) {
            call.started = true;
            if call.name == "write" {
                call.before = call.input["path"].as_str().map(|p| std::fs::read_to_string(p).unwrap_or_default());
            }
            let mut title = title(&call.name, &call.input);
            if title.is_empty() {
                title = t["title"].as_str().filter(|s| *s != call.name).unwrap_or("").to_string();
            }
            let ev = Ev::ToolStart { id: id.to_string(), name: call.name.clone(), title, input: call.input.clone() };
            let _ = self.events.send(ev);
        }
    }

    fn on_update(&mut self, p: &Value) {
        // Subagents' sessions (OpenCode's child-session updates) aren't shown.
        if p["sessionId"].as_str().is_some_and(|s| Some(s) != self.session.as_deref()) {
            return;
        }
        let u = &p["update"];
        let text = || u["content"]["text"].as_str().unwrap_or("").to_string();
        let id = || u["messageId"].as_str().unwrap_or("msg").to_string();
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                let text = text();
                if self.turn_tokens.streamed(&text) {
                    self.emit_usage();
                }
                self.emit(Ev::TextDelta { id: id(), text });
            }
            "agent_thought_chunk" => {
                let text = text();
                if self.turn_tokens.streamed_sub(&text) {
                    self.emit_usage();
                }
                self.emit(Ev::ReasoningDelta { id: id(), text });
            }
            "tool_call" | "tool_call_update" => self.tool_update(u),
            "plan" => {
                let todos = u["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| Todo { content: e["content"].as_str().unwrap_or("").to_string(), status: e["status"].as_str().unwrap_or("pending").to_string() })
                    .collect();
                self.emit(Ev::Todos(todos));
            }
            "usage_update" => {
                self.usage.context = u["used"].as_u64().unwrap_or(self.usage.context);
                self.usage.window = u["size"].as_u64().unwrap_or(self.usage.window);
                if let Some(cost) = u["cost"]["amount"].as_f64() {
                    self.usage.cost = Some(cost);
                }
                self.emit_usage();
            }
            "available_commands_update" => {
                let skills = u["availableCommands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| Some(Skill { name: c["name"].as_str()?.to_string(), description: c["description"].as_str().unwrap_or("").to_string() }))
                    .collect();
                self.emit(Ev::Skills(skills));
            }
            "config_option_update" => {
                if u["configOptions"].is_array() {
                    self.config = u["configOptions"].clone();
                    self.show_model();
                }
            }
            "session_info_update" => match u["_meta"]["opencode/compaction"]["status"].as_str() {
                Some("started") => self.emit(Ev::Notice("Compacting the conversation…".into())),
                Some("completed") => self.emit(Ev::Notice("Conversation compacted.".into())),
                _ => {}
            },
            _ => {}
        }
    }

    fn tool_update(&mut self, u: &Value) {
        self.track(u, false);
        let Some(id) = u["toolCallId"].as_str() else { return };
        let Some(call) = self.calls.get_mut(id) else { return };
        let output = content_text(&u["content"]);
        let status = u["status"].as_str().unwrap_or("");
        if status == "in_progress" && output.len() > call.shown && output.is_char_boundary(call.shown) {
            let chunk = output[call.shown..].to_string();
            call.shown = output.len();
            let _ = self.events.send(Ev::ToolOutput { id: id.to_string(), chunk });
            return;
        }
        if !matches!(status, "completed" | "failed") {
            return;
        }
        let Some(call) = self.calls.remove(id) else { return };
        let ok = status == "completed";
        let output = if output.is_empty() { u["rawOutput"]["error"].as_str().unwrap_or("").to_string() } else { output };
        let diff = if ok { edit_diff(&call, u) } else { None };
        self.emit(Ev::ToolEnd { id: id.to_string(), ok, output, diff });
    }
}

/// The text parts of a tool call's ACP `content`.
fn content_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["type"] == "content")
        .filter_map(|c| c["content"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The diff parts of a tool call's ACP `content` (`{ type: "diff", path, oldText, newText }`).
fn content_diff(content: &Value) -> Option<String> {
    let out: String = content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["type"] == "diff")
        .map(|c| diff::unified(&short_path(c["path"].as_str().unwrap_or("")), c["oldText"].as_str().unwrap_or(""), c["newText"].as_str().unwrap_or("")))
        .collect();
    (!out.is_empty()).then_some(out)
}

/// The diff a finished edit made: OpenCode's own, else from what its input says.
fn edit_diff(call: &Call, u: &Value) -> Option<String> {
    if let Some(d) = content_diff(&u["content"]) {
        return Some(d);
    }
    let patches: String = u["rawOutput"]["metadata"]["files"].as_array().into_iter().flatten().filter_map(|f| f["patch"].as_str()).collect();
    if !patches.is_empty() {
        return Some(patches);
    }
    if call.diff.is_some() {
        return call.diff.clone();
    }
    let input = &call.input;
    let path = short_path(input["path"].as_str().or(input["filePath"].as_str())?);
    match call.name.as_str() {
        "edit" => Some(diff::unified(&path, input["oldString"].as_str()?, input["newString"].as_str()?)),
        "write" => Some(diff::unified(&path, call.before.as_deref().unwrap_or(""), input["content"].as_str()?)),
        _ => None,
    }
}

/// A one-line summary of an OpenCode tool call for its card.
pub fn title(name: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let path = || short_path(&input["path"].as_str().or(input["filePath"].as_str()).unwrap_or("").to_string());
    let t = match name {
        "shell" | "bash" => s("command"),
        "read" | "write" | "edit" | "multiedit" | "list" => path(),
        "grep" => format!("{} {}", s("pattern"), path()).trim_end().to_string(),
        "glob" => s("pattern"),
        "webfetch" => s("url"),
        "websearch" | "codesearch" => s("query"),
        "task" => s("description"),
        "skill" => s("name"),
        "todowrite" | "todoread" => "update todos".into(),
        "apply_patch" | "patch" => s("patchText")
            .lines()
            .filter_map(|l| ["*** Update File: ", "*** Add File: ", "*** Delete File: "].iter().find_map(|p| l.strip_prefix(p)))
            .map(short_path)
            .collect::<Vec<_>>()
            .join(", "),
        n if n.contains("winfer") && n.ends_with("bg") => crate::harness::tools::title("bg", input),
        _ => input.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string))).unwrap_or_default(),
    };
    t.lines().next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge(mode: Mode) -> (Bridge, Receiver<Ev>) {
        let (tx, rx) = channel();
        let opts = Opts {
            cwd: "/tmp".into(),
            model: "opencode/big-pickle".into(),
            effort: None,
            mode,
            resume: None,
            system: Some("Be brief.".into()),
            mcp: Default::default(),
            allowed_commands: vec!["ls".into()],
            trim_at: 0,
            compact_at: 0,
            lean: false,
            skills: crate::harness::claude_skills::LeanSkills::All,
            bg_owner: String::new(),
            sub_model: None,
            sub_effort: None,
        };
        let mut b = Bridge::new(&opts, tx);
        b.session = Some("ses_1".into());
        (b, rx)
    }

    fn update(update: Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "ses_1", "update": update } })
    }

    #[test]
    fn labels_model_ids() {
        assert_eq!(label("gpt-5.6-luna-fast"), "GPT-5.6 Luna Fast");
        assert_eq!(label("big-pickle"), "Big Pickle");
        assert_eq!(label("glm-5.3-flash-free"), "GLM-5.3 Flash Free");
    }

    #[test]
    fn asks_for_permission_unless_the_mode_allows() {
        let (mut b, rx) = bridge(Mode::Ask);
        let req = json!({ "jsonrpc": "2.0", "id": 0, "method": "session/request_permission", "params": { "sessionId": "ses_1",
            "toolCall": { "toolCallId": "c1", "name": "write", "kind": "edit", "rawInput": { "path": "/tmp/x.txt", "content": "hi" },
                "content": [{ "type": "diff", "path": "/tmp/x.txt", "oldText": null, "newText": "hi\n" }] },
            "options": [{ "optionId": "once", "kind": "allow_once" }, { "optionId": "always", "kind": "allow_always" }, { "optionId": "reject", "kind": "reject_once" }] } });
        b.on_message(&req);
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::ToolStart { name, .. } if name == "write"));
        let Some(Ev::Approval(a)) = evs.get(1) else { panic!("{evs:?}") };
        assert!(a.diff.as_deref().is_some_and(|d| d.contains("+hi")));
        b.on_cmd(Cmd::Approve(a.id.clone(), Decision::Deny));
        assert_eq!(b.outbox.pop().unwrap()["result"]["outcome"]["optionId"], "reject");

        let (mut b, rx) = bridge(Mode::AutoEdit);
        b.on_message(&req);
        assert_eq!(b.outbox.pop().unwrap()["result"]["outcome"]["optionId"], "once");
        assert!(!rx.try_iter().any(|e| matches!(e, Ev::Approval(_))));
    }

    #[test]
    fn maps_a_turn() {
        let (mut b, rx) = bridge(Mode::Yolo);
        b.on_cmd(Cmd::Send(UserMsg { text: "hello".into(), ..Default::default() }));
        let prompt = b.outbox.pop().unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "Be brief.\n\nhello");
        b.on_message(&update(json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "type": "text", "text": "pong" } })));
        b.on_message(&update(json!({ "sessionUpdate": "tool_call", "toolCallId": "c1", "name": "shell", "kind": "execute", "status": "pending", "rawInput": {} })));
        b.on_message(&update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress", "rawInput": { "command": "ls" } })));
        b.on_message(&update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "a.txt\n" } }] })));
        b.on_message(&update(json!({ "sessionUpdate": "usage_update", "used": 100, "size": 1000, "cost": { "amount": 0.5 } })));
        b.on_message(&json!({ "jsonrpc": "2.0", "id": prompt["id"], "result": { "stopReason": "end_turn", "usage": { "inputTokens": 10, "outputTokens": 4, "cachedReadTokens": 90 } } }));
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::TurnStart));
        assert!(evs.iter().any(|e| matches!(e, Ev::TextDelta { text, .. } if text == "pong")));
        assert!(evs.iter().any(|e| matches!(e, Ev::ToolStart { title, .. } if title == "ls")));
        assert!(evs.iter().any(|e| matches!(e, Ev::ToolEnd { ok: true, output, .. } if output == "a.txt\n")));
        assert!(evs.iter().any(|e| matches!(e, Ev::Usage(u) if u.input == 100 && u.cached == 90 && u.context == 100 && u.window == 1000)));
        assert!(matches!(evs.last(), Some(Ev::TurnDone)));
        assert!(!b.busy);
    }

    #[test]
    fn mcp_servers_become_acp_servers() {
        let mut servers = serde_json::Map::new();
        servers.insert("winfer".into(), json!({ "command": "/bin/winfer", "args": ["mcp-stdio"], "env": { "A": "1" } }));
        let acp = mcp_servers(&servers);
        assert_eq!(acp[0], json!({ "name": "winfer", "command": "/bin/winfer", "args": ["mcp-stdio"], "env": [{ "name": "A", "value": "1" }] }));
    }

    #[test]
    fn permissions_ask_on_top_of_the_users_config() {
        let config: Value = serde_json::from_str(&config_env(Some(r#"{"theme":"dark","permission":{"read":"allow"}}"#))).unwrap();
        assert_eq!(config["theme"], "dark");
        assert_eq!(config["permission"], json!({ "read": "allow", "edit": "ask", "bash": "ask", "webfetch": "ask" }));
    }
}
