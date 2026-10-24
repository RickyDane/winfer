//! Pi bridge: `pi --mode rpc` (JSON lines over stdio), with pi's own tools, models and logins.
//! Pi never asks before a tool call, so winfer loads a small extension into it ([`EXTENSION`])
//! that holds every call until the bridge answers: on its own per the permission mode, or with
//! the user's decision. The extension also connects winfer's MCP servers and adds a plan tool.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Question, Skill, Todo, TurnTokens, Usage, UserMsg};
use crate::harness::tools::{self, Risk};

/// The models pi has logins for, as `pi/<provider>/<model>` specs; empty when it isn't installed.
pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("pi").is_none() {
        return Vec::new();
    }
    let (tx, rx) = channel();
    let args: Vec<String> = ["--mode", "rpc", "--no-session", "--no-extensions", "--no-skills", "--no-mcp", "--offline"].map(String::from).into();
    let Ok(mut proc) = LineProcess::spawn("pi", &args, &[], &std::env::temp_dir(), tx) else {
        return Vec::new();
    };
    let _ = proc.write(&json!({ "id": "models", "type": "get_available_models" }));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(In::Line(line)) = rx.recv_timeout(left) else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["id"] == "models" {
            return v["data"]["models"].as_array().into_iter().flatten().filter_map(model_info).collect();
        }
    }
    Vec::new()
}

fn model_info(m: &Value) -> Option<ModelInfo> {
    let (provider, id) = (m["provider"].as_str()?, m["id"].as_str()?);
    Some(ModelInfo {
        spec: format!("pi/{provider}/{id}"),
        label: m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
        detail: format!("Pi · {provider}"),
        efforts: thinking_levels(m),
    })
}

/// The `--thinking` levels pi takes for a model, as its `getSupportedThinkingLevels` works them
/// out: a level mapped to null is off the list, and xhigh/max are on it only when mapped. "off"
/// is left out (no effort set is the model's default).
fn thinking_levels(m: &Value) -> Option<Vec<String>> {
    if !m["reasoning"].as_bool()? {
        return Some(Vec::new());
    }
    let map = &m["thinkingLevelMap"];
    let levels = ["minimal", "low", "medium", "high", "xhigh", "max"].into_iter().filter(|l| match map.get(*l) {
        Some(Value::Null) => false,
        Some(_) => true,
        None => !matches!(*l, "xhigh" | "max"),
    });
    Some(levels.map(String::from).collect())
}

/// winfer's extension for pi (see the module docs). Tool calls wait on an extension UI `select`
/// whose title is the call as JSON; the answer is "allow" or "deny:<reason for the model>".
const EXTENSION: &str = r#"import { Type } from "typebox";

export default function (pi) {
  for (const [name, config] of Object.entries(JSON.parse(process.env.WINFER_PI_MCP || "{}"))) {
    try {
      pi.registerMcpServer(name, { exposure: "direct", ...config });
    } catch (e) {
      console.error(`winfer: MCP server ${name}: ${e}`);
    }
  }
  pi.on("tool_call", async (event, ctx) => {
    const call = JSON.stringify({ winfer: "tool_call", tool: event.toolName, input: event.input });
    const answer = await ctx.ui.select(call, ["allow", "deny"]);
    if (answer === "allow") return undefined;
    return { block: true, reason: answer?.startsWith("deny:") ? answer.slice(5) : "The user declined this tool call." };
  });
  pi.registerTool({
    name: "update_plan",
    label: "Plan",
    description: "Keep a short plan for multi-step work: send the whole list of steps each time, with at most one in_progress, and mark steps completed as you finish them. The user sees it live.",
    parameters: Type.Object({
      explanation: Type.Optional(Type.String()),
      plan: Type.Array(Type.Object({
        step: Type.String(),
        status: Type.Union([Type.Literal("pending"), Type.Literal("in_progress"), Type.Literal("completed")]),
      })),
    }),
    async execute() {
      return { content: [{ type: "text", text: "Plan updated." }], details: {} };
    },
  });
}
"#;

/// Writes [`EXTENSION`] where pi can load it.
fn extension_file() -> Result<PathBuf, String> {
    let dir = crate::harness::config::dir().join("pi");
    let path = dir.join("winfer-extension.js");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION) {
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, EXTENSION)).map_err(|e| format!("couldn't write pi's extension: {e}"))?;
    }
    Ok(path)
}

fn args(opts: &Opts, extension: &std::path::Path) -> Vec<String> {
    let mut args: Vec<String> = vec!["--mode".into(), "rpc".into(), "-e".into(), extension.to_string_lossy().into_owned()];
    if !opts.model.is_empty() {
        args.extend(["--model".into(), opts.model.clone()]);
    }
    if let Some(effort) = &opts.effort {
        args.extend(["--thinking".into(), effort.clone()]);
    }
    if let Some(session) = &opts.resume {
        args.extend(["--session".into(), session.clone()]);
    }
    if let Some(system) = &opts.system {
        args.extend(["--append-system-prompt".into(), system.clone()]);
    }
    args
}

const PLAN_DENIED: &str = "Plan mode is on: files can't be edited. Describe the change in the plan instead.";

/// What a tool call needs before it runs.
enum Verdict {
    Allow,
    Deny(&'static str),
    Ask,
}

/// An extension UI request waiting on the user, by its id.
enum Ask {
    /// A tool call; `key` is what "always allow" remembers.
    Tool { key: String },
    Select,
    Confirm,
    Input,
}

/// A running tool call.
struct Call {
    name: String,
    input: Value,
    /// For edits: the file and what it held before (None: it didn't exist).
    before: Option<(PathBuf, Option<String>)>,
    /// The output shown so far (pi sends partial results whole).
    shown: String,
}

#[derive(Default)]
struct Live {
    /// Session totals, the context and the running turn's counts.
    usage: Usage,
    turn: TurnTokens,
    /// The running turn's output in finished requests.
    done: u64,
}

struct Bridge {
    events: Events,
    proc: LineProcess,
    cwd: PathBuf,
    mode: Mode,
    bg_owner: String,
    /// "Always allow" keys, seeded with the configured commands.
    allowed: HashSet<String>,
    asks: HashMap<String, Ask>,
    calls: HashMap<String, Call>,
    busy: bool,
    /// Counts assistant messages, for text ids.
    message: u64,
    live: Live,
    /// The files each user turn edited, as they were before it; newest last, for /undo.
    turns: Vec<Vec<(PathBuf, Option<String>)>>,
    /// An undo in flight: whether to put its files back.
    undoing: Option<bool>,
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let proc = extension_file().and_then(|ext| {
        let mcp = serde_json::Value::Object(opts.mcp.clone()).to_string();
        let env: &[(&str, &str)] = if opts.mcp.is_empty() { &[] } else { &[("WINFER_PI_MCP", &mcp)] };
        LineProcess::spawn("pi", &args(&opts, &ext), env, &opts.cwd, me)
    });
    let proc = match proc {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(Ev::Exited(e));
            return;
        }
    };
    let mut b = Bridge {
        events,
        proc,
        cwd: opts.cwd.clone(),
        mode: opts.mode,
        bg_owner: opts.bg_owner.clone(),
        allowed: opts.allowed_commands.iter().map(|c| format!("bash:{c}")).collect(),
        asks: HashMap::new(),
        calls: HashMap::new(),
        busy: false,
        message: 0,
        live: Live::default(),
        turns: Vec::new(),
        undoing: None,
    };
    crate::harness::bg::set_mode(&b.bg_owner, b.mode);
    for (id, kind) in [("state", "get_state"), ("stats", "get_session_stats"), ("commands", "get_commands")] {
        b.write(json!({ "id": id, "type": kind }));
    }
    b.write(json!({ "type": "set_auto_compaction", "enabled": opts.compact_at > 0 }));
    while let Some(msg) = super::recv(&rx) {
        match msg {
            In::Line(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    b.on_line(&v);
                }
            }
            In::Closed => {
                let reason = b.proc.exit_reason();
                let _ = b.events.send(Ev::Exited(format!("Pi stopped: {reason}")));
                return;
            }
            In::Cmd(Cmd::Shutdown) => return,
            In::Cmd(cmd) => b.on_cmd(cmd),
        }
    }
}

impl Bridge {
    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn write(&mut self, v: Value) {
        if let Err(e) = self.proc.write(&v) {
            self.emit(Ev::Error(format!("couldn't reach pi: {e}")));
        }
    }

    fn emit_usage(&mut self) {
        let l = &mut self.live;
        l.usage.turn = l.turn.now();
        l.usage.turn_streamed = l.turn.streamed_total();
        let u = l.usage.clone();
        self.emit(Ev::Usage(u));
    }

    fn turn_done(&mut self) {
        self.busy = false;
        self.emit(Ev::TurnDone);
        self.write(json!({ "id": "stats", "type": "get_session_stats" }));
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Send(msg) => {
                self.busy = true;
                self.turns.push(Vec::new());
                let l = &mut self.live;
                (l.turn, l.done, l.usage.turn_input, l.usage.turn_cached) = (TurnTokens::default(), 0, 0, 0);
                self.emit(Ev::TurnStart);
                self.write(prompt("prompt", &msg));
            }
            // Pi delivers it after the tool calls in flight, before its next request.
            Cmd::Steer(msg) if self.busy => self.write(prompt("steer", &msg)),
            Cmd::Steer(msg) => self.on_cmd(Cmd::Send(msg)),
            Cmd::Interrupt => self.write(json!({ "type": "abort" })),
            Cmd::Approve(id, decision) => match self.asks.remove(&id) {
                Some(Ask::Tool { key }) => {
                    if decision == Decision::Always {
                        self.allowed.insert(key);
                    }
                    let value = if decision == Decision::Deny { "deny:The user declined this. Ask them how to proceed." } else { "allow" };
                    self.write(json!({ "type": "extension_ui_response", "id": id, "value": value }));
                }
                Some(Ask::Confirm) => self.write(json!({ "type": "extension_ui_response", "id": id, "confirmed": decision != Decision::Deny })),
                Some(_) => self.write(json!({ "type": "extension_ui_response", "id": id, "cancelled": true })),
                None => {}
            },
            Cmd::Answer(id, answers) => {
                if self.asks.remove(&id).is_some() {
                    let value = answers.into_iter().next().unwrap_or_default();
                    self.write(json!({ "type": "extension_ui_response", "id": id, "value": value }));
                }
            }
            Cmd::SetModel(model) => match model.split_once('/') {
                Some((provider, id)) => self.write(json!({ "id": "model", "type": "set_model", "provider": provider, "modelId": id })),
                None => self.emit(Ev::Notice(format!("Pi needs a model as provider/model, not “{model}”."))),
            },
            // Pi's default thinking level is medium.
            Cmd::SetEffort(e) => {
                let level = e.unwrap_or_else(|| "medium".into());
                self.write(json!({ "id": "thinking", "type": "set_thinking_level", "level": level }));
            }
            Cmd::SetMode(mode) => {
                self.mode = mode;
                crate::harness::bg::set_mode(&self.bg_owner, mode);
            }
            Cmd::Compact => {
                self.busy = true;
                self.emit(Ev::TurnStart);
                self.write(json!({ "id": "compact", "type": "compact" }));
            }
            // Pi compacts on its own when the context nearly fills; it has no micro-compaction.
            Cmd::SetSubagent { .. } => {}
            Cmd::SetCompaction { compact, .. } => self.write(json!({ "type": "set_auto_compaction", "enabled": compact > 0 })),
            Cmd::Undo { files } => {
                if self.busy || self.undoing.is_some() {
                    self.emit(Ev::Undone(Err("Wait for the turn to finish (or press Esc) before /undo.".into())));
                } else {
                    self.undoing = Some(files);
                    self.write(json!({ "id": "undo", "type": "get_fork_messages" }));
                }
            }
            Cmd::StopTask(id) if crate::harness::bg::stop(&id) => {}
            Cmd::StopTask(_) => self.emit(Ev::Notice("Pi has no background tasks to stop.".into())),
            Cmd::Shutdown => {}
        }
    }

    fn on_line(&mut self, v: &Value) {
        match v["type"].as_str().unwrap_or("") {
            "response" => self.on_response(v),
            // Pi runs on its own too (queued follow-ups, extensions): show it as a turn.
            "agent_start" if !self.busy => {
                self.busy = true;
                self.emit(Ev::TurnStart);
            }
            "agent_settled" => self.turn_done(),
            "message_start" if v["message"]["role"] == "assistant" => self.message += 1,
            "message_update" => self.on_update(&v["assistantMessageEvent"]),
            "message_end" if v["message"]["role"] == "assistant" => self.on_message_end(&v["message"]),
            "tool_execution_start" => self.on_tool_start(v),
            "tool_execution_update" => {
                let id = v["toolCallId"].as_str().unwrap_or("").to_string();
                let text = content_text(&v["partialResult"]["content"]);
                let Some(call) = self.calls.get_mut(&id) else { return };
                // Partial results come whole: send what's new.
                let chunk = match text.strip_prefix(call.shown.as_str()) {
                    Some(rest) => rest.to_string(),
                    None => return,
                };
                call.shown = text;
                if !chunk.is_empty() {
                    self.emit(Ev::ToolOutput { id, chunk });
                }
            }
            "tool_execution_end" => self.on_tool_end(v),
            "extension_ui_request" => self.on_ui_request(v),
            "compaction_end" => match (v["result"].is_object(), v["errorMessage"].as_str()) {
                (true, _) => self.emit(Ev::Notice("Context compacted.".into())),
                (false, Some(e)) => self.emit(Ev::Notice(format!("Couldn't compact the context: {e}"))),
                _ => {}
            },
            "auto_retry_start" => {
                let n = |k: &str| v[k].as_u64().unwrap_or(0);
                let why = v["errorMessage"].as_str().unwrap_or("");
                self.emit(Ev::Notice(format!("Retrying ({}/{}): {why}", n("attempt"), n("maxAttempts"))));
            }
            "extension_error" => {
                let e = v["error"].as_str().unwrap_or("");
                self.emit(Ev::Notice(format!("Pi extension error ({}): {e}", v["extensionPath"].as_str().unwrap_or(""))));
            }
            _ => {}
        }
    }

    fn on_response(&mut self, v: &Value) {
        let id = v["id"].as_str().unwrap_or("");
        let data = &v["data"];
        if v["success"] != true {
            let error = v["error"].as_str().unwrap_or("failed").to_string();
            match id {
                "prompt" | "compact" => {
                    self.emit(Ev::Error(format!("Pi: {error}")));
                    self.turn_done();
                }
                "undo" | "fork" => {
                    self.undoing = None;
                    self.emit(Ev::Undone(Err(format!("Pi couldn't undo the turn: {error}"))));
                }
                _ => self.emit(Ev::Notice(format!("Pi: {error}"))),
            }
            return;
        }
        match id {
            "state" => {
                if let Some(s) = data["sessionId"].as_str() {
                    self.emit(Ev::Session(s.to_string()));
                }
                self.on_model(&data["model"]);
            }
            "model" => self.on_model(data),
            "stats" => {
                let t = &data["tokens"];
                let n = |k: &str| t[k].as_u64().unwrap_or(0);
                let u = &mut self.live.usage;
                (u.input, u.cached, u.output, u.cost) = (n("input") + n("cacheRead") + n("cacheWrite"), n("cacheRead"), n("output"), data["cost"].as_f64());
                if let Some(c) = data["contextUsage"]["tokens"].as_u64() {
                    u.context = c;
                }
                if let Some(w) = data["contextUsage"]["contextWindow"].as_u64() {
                    u.window = w;
                }
                self.emit_usage();
            }
            "commands" => {
                let skills = data["commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["source"] == "skill")
                    .filter_map(|c| {
                        let name = c["name"].as_str()?.strip_prefix("skill:")?;
                        Some(Skill { name: name.to_string(), description: c["description"].as_str().unwrap_or("").to_string() })
                    })
                    .collect();
                self.emit(Ev::Skills(skills));
            }
            // An extension command handled it; no run follows.
            "prompt" if data["disposition"] == "handled" => self.turn_done(),
            "compact" => self.turn_done(),
            "undo" => match data["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["entryId"].as_str()) {
                Some(entry) => self.write(json!({ "id": "fork", "type": "fork", "entryId": entry })),
                None => {
                    self.undoing = None;
                    self.emit(Ev::Undone(Err("Nothing to undo.".into())));
                }
            },
            // The fork is a new session that ends before the last user message.
            "fork" => {
                let files = self.undoing.take().unwrap_or(false);
                if data["cancelled"] == true {
                    return self.emit(Ev::Undone(Err("A pi extension cancelled the undo.".into())));
                }
                let turn = self.turns.pop().unwrap_or_default();
                let names = if files { revert(&turn) } else { Vec::new() };
                self.emit(Ev::Undone(Ok(names)));
                self.write(json!({ "id": "state", "type": "get_state" }));
                self.write(json!({ "id": "stats", "type": "get_session_stats" }));
            }
            _ => {}
        }
    }

    fn on_model(&mut self, m: &Value) {
        if let (Some(provider), Some(id)) = (m["provider"].as_str(), m["id"].as_str()) {
            self.emit(Ev::Model(format!("{provider}/{id}")));
        }
        if let Some(w) = m["contextWindow"].as_u64() {
            self.live.usage.window = w;
        }
    }

    fn on_update(&mut self, e: &Value) {
        let id = format!("m{}:{}", self.message, e["contentIndex"].as_u64().unwrap_or(0));
        let delta = e["delta"].as_str().unwrap_or("");
        if !delta.is_empty() && self.live.turn.streamed(delta) {
            self.emit_usage();
        }
        match e["type"].as_str().unwrap_or("") {
            "text_delta" => self.emit(Ev::TextDelta { id, text: delta.to_string() }),
            "text_end" => self.emit(Ev::Text { id, text: e["content"].as_str().unwrap_or("").to_string() }),
            "thinking_delta" => self.emit(Ev::ReasoningDelta { id, text: delta.to_string() }),
            _ => {}
        }
    }

    /// A finished model request: its exact usage, or its error.
    fn on_message_end(&mut self, m: &Value) {
        let u = &m["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let input = n("input") + n("cacheRead") + n("cacheWrite");
        let l = &mut self.live;
        l.usage.input += input;
        l.usage.cached += n("cacheRead");
        l.usage.output += n("output");
        l.usage.turn_input += input;
        l.usage.turn_cached += n("cacheRead");
        if let Some(c) = u["cost"]["total"].as_f64() {
            l.usage.cost = Some(l.usage.cost.unwrap_or(0.0) + c);
        }
        if input > 0 {
            l.usage.context = input + n("output");
        }
        l.done += n("output");
        l.turn.exact(l.done);
        self.emit_usage();
        if m["stopReason"] == "error" {
            self.emit(Ev::Error(m["errorMessage"].as_str().unwrap_or("Pi reported an error.").to_string()));
        }
    }

    fn on_tool_start(&mut self, v: &Value) {
        let id = v["toolCallId"].as_str().unwrap_or("").to_string();
        let name = v["toolName"].as_str().unwrap_or("tool").to_string();
        let input = v["args"].clone();
        if name == "update_plan" {
            self.emit(Ev::Todos(plan_todos(&input)));
        }
        let before = matches!(name.as_str(), "edit" | "write").then(|| input["path"].as_str()).flatten().map(|p| {
            let path = tools::resolve(&self.cwd, p);
            let old = std::fs::read_to_string(&path).ok();
            (path, old)
        });
        self.emit(Ev::ToolStart { id: id.clone(), name: display_name(&name), title: title(&name, &input), input: input.clone() });
        self.calls.insert(id, Call { name, input, before, shown: String::new() });
    }

    fn on_tool_end(&mut self, v: &Value) {
        let id = v["toolCallId"].as_str().unwrap_or("").to_string();
        let ok = v["isError"] != true;
        let output = content_text(&v["result"]["content"]);
        let call = self.calls.remove(&id);
        let diff = call.as_ref().filter(|_| ok).and_then(|c| match c.name.as_str() {
            "edit" => v["result"]["details"]["patch"].as_str().map(str::to_string),
            "write" => {
                let short = tools::short_path(c.input["path"].as_str()?);
                let old = c.before.as_ref().and_then(|(_, old)| old.clone()).unwrap_or_default();
                Some(diff::unified(&short, &old, c.input["content"].as_str()?))
            }
            _ => None,
        });
        // The turn's first change to a file keeps what it held before, for /undo.
        if let (true, Some((path, old)), Some(turn)) = (ok, call.and_then(|c| c.before), self.turns.last_mut()) {
            if !turn.iter().any(|(p, _)| *p == path) {
                turn.push((path, old));
            }
        }
        self.emit(Ev::ToolEnd { id, ok, output, diff });
    }

    fn on_ui_request(&mut self, v: &Value) {
        let id = v["id"].as_str().unwrap_or("").to_string();
        let title = v["title"].as_str().unwrap_or("").to_string();
        match v["method"].as_str().unwrap_or("") {
            "select" => match serde_json::from_str::<Value>(&title).ok().filter(|c| c["winfer"] == "tool_call") {
                Some(call) => self.on_tool_call(id, &call),
                None => {
                    let options = v["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str()).map(|o| (o.to_string(), String::new())).collect();
                    self.question(id, Ask::Select, Question { header: String::new(), question: title, options, multi: false });
                }
            },
            "input" | "editor" => self.question(id, Ask::Input, Question { header: String::new(), question: title, options: Vec::new(), multi: false }),
            "confirm" => {
                self.asks.insert(id.clone(), Ask::Confirm);
                let detail = v["message"].as_str().unwrap_or("").to_string();
                self.emit(Ev::Approval(Approval { id, tool: "Confirm".into(), title, detail, ..Default::default() }));
            }
            "notify" => self.emit(Ev::Notice(v["message"].as_str().unwrap_or("").to_string())),
            _ => {}
        }
    }

    fn question(&mut self, id: String, kind: Ask, q: Question) {
        self.asks.insert(id.clone(), kind);
        self.emit(Ev::Approval(Approval { id, tool: "Question".into(), title: q.question.clone(), questions: vec![q], ..Default::default() }));
    }

    /// The extension holds a tool call: answer it per the mode, or ask the user.
    fn on_tool_call(&mut self, id: String, call: &Value) {
        let name = call["tool"].as_str().unwrap_or("");
        let input = &call["input"];
        let key = allow_key(name, input);
        let answer = match verdict(self.mode, name, input, self.allowed.contains(&key)) {
            Verdict::Allow => "allow".to_string(),
            Verdict::Deny(why) => format!("deny:{why}"),
            Verdict::Ask => {
                self.asks.insert(id.clone(), Ask::Tool { key });
                let diff = edit_diff(&self.cwd, name, input);
                return self.emit(Ev::Approval(Approval { id, tool: display_name(name), title: title(name, input), diff, ..Default::default() }));
            }
        };
        self.write(json!({ "type": "extension_ui_response", "id": id, "value": answer }));
    }
}

/// How much a pi tool may do; MCP tools count as commands, except winfer's `bg` reads.
fn risk(name: &str, input: &Value) -> Risk {
    match name {
        "read" | "grep" | "find" | "ls" | "update_plan" | "tool_search" => Risk::Read,
        // Its scripts' tool calls come through here one by one.
        "codemode" => Risk::Read,
        "edit" | "write" => Risk::Edit,
        n if is_bg(n) => tools::risk("bg", input),
        _ => Risk::Exec,
    }
}

fn verdict(mode: Mode, name: &str, input: &Value, always: bool) -> Verdict {
    match (risk(name, input), mode) {
        (Risk::Read, _) => Verdict::Allow,
        (Risk::Edit, m) if m.plans() => Verdict::Deny(PLAN_DENIED),
        (_, Mode::Yolo | Mode::YoloPlan) | (Risk::Edit, Mode::AutoEdit) => Verdict::Allow,
        _ if always => Verdict::Allow,
        _ => Verdict::Ask,
    }
}

/// What "always allow" covers: a command's program, else the tool.
fn allow_key(name: &str, input: &Value) -> String {
    let program = |k: &str| input[k].as_str().unwrap_or("").split_whitespace().next().unwrap_or("").to_string();
    match name {
        "bash" | "powershell" => format!("bash:{}", program("command")),
        n if is_bg(n) => format!("bg:{}", program("cmd")),
        n => n.to_string(),
    }
}

/// winfer's own `bg` tool, through its MCP server.
fn is_bg(name: &str) -> bool {
    name.starts_with("mcp__winfer") && name.ends_with("__bg")
}

/// Puts back the files a turn changed; returns their names.
fn revert(turn: &[(PathBuf, Option<String>)]) -> Vec<String> {
    let mut names: Vec<String> = turn
        .iter()
        .filter(|(path, old)| match old {
            Some(text) => std::fs::write(path, text).is_ok(),
            None => std::fs::remove_file(path).is_ok(),
        })
        .map(|(path, _)| tools::short_path(&path.to_string_lossy()))
        .collect();
    names.sort();
    names
}

fn prompt(kind: &str, msg: &UserMsg) -> Value {
    // Pi runs `/skill:name args` as that skill.
    let message = match &msg.skill {
        Some(skill) => format!("/skill:{skill} {}", msg.text).trim_end().to_string(),
        None => msg.text.clone(),
    };
    let images: Vec<Value> = msg
        .images
        .iter()
        .filter_map(|p| tools::image_base64(p))
        .map(|(media, data)| json!({ "type": "image", "data": data, "mimeType": media }))
        .collect();
    json!({ "id": kind, "type": kind, "message": message, "images": images })
}

/// `update_plan` arguments as todos.
fn plan_todos(input: &Value) -> Vec<Todo> {
    input["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Todo { content: s["step"].as_str().unwrap_or("").to_string(), status: s["status"].as_str().unwrap_or("pending").to_string() })
        .filter(|t| !t.content.is_empty())
        .collect()
}

fn content_text(content: &Value) -> String {
    content.as_array().into_iter().flatten().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")
}

/// Card names like the other backends' (the plan tool's "Todos" gets no card).
fn display_name(name: &str) -> String {
    match name {
        "update_plan" => "Todos".into(),
        "find" => "Glob".into(),
        "ls" => "LS".into(),
        n if n.starts_with("mcp__") => n["mcp__".len()..].replacen("__", " · ", 1),
        n => {
            let mut c = n.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

fn title(name: &str, input: &Value) -> String {
    match name {
        "find" => tools::title("glob", input),
        "update_plan" => "update plan".into(),
        n if is_bg(n) => tools::title("bg", input),
        n => tools::title(n, input),
    }
}

/// The diff an edit would make, from its input (for approvals).
fn edit_diff(cwd: &std::path::Path, name: &str, input: &Value) -> Option<String> {
    let path = input["path"].as_str()?;
    let short = tools::short_path(path);
    match name {
        "edit" => {
            let edits = input["edits"].as_array()?;
            Some(edits.iter().filter_map(|e| Some(diff::unified(&short, e["oldText"].as_str()?, e["newText"].as_str()?))).collect())
        }
        "write" => {
            let old = std::fs::read_to_string(tools::resolve(cwd, path)).unwrap_or_default();
            Some(diff::unified(&short, &old, input["content"].as_str()?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_levels_follow_the_level_map() {
        let m = |v: Value| thinking_levels(&v);
        assert_eq!(m(json!({ "reasoning": false })), Some(vec![]));
        assert_eq!(m(json!({ "reasoning": true })), Some(vec!["minimal".to_string(), "low".to_string(), "medium".to_string(), "high".to_string()]));
        assert_eq!(m(json!({ "reasoning": true, "thinkingLevelMap": { "minimal": null, "xhigh": "xhigh", "max": "max" } })), Some(vec!["low".to_string(), "medium".to_string(), "high".to_string(), "xhigh".to_string(), "max".to_string()]));
        assert_eq!(m(json!({ "reasoning": true, "thinkingLevelMap": { "max": "max" } })), Some(vec!["minimal".to_string(), "low".to_string(), "medium".to_string(), "high".to_string(), "max".to_string()]));
        assert_eq!(m(json!({})), None);
    }

    #[test]
    fn modes_decide_tool_calls() {
        let bash = json!({ "command": "cargo test" });
        let allowed = |mode, name, always| matches!(verdict(mode, name, &bash, always), Verdict::Allow);
        assert!(allowed(Mode::Ask, "read", false));
        assert!(!allowed(Mode::Ask, "bash", false));
        assert!(allowed(Mode::Ask, "bash", true));
        assert!(allowed(Mode::AutoEdit, "edit", false));
        assert!(!allowed(Mode::AutoEdit, "bash", false));
        assert!(allowed(Mode::YoloPlan, "bash", false));
        assert!(matches!(verdict(Mode::Yolo, "write", &bash, false), Verdict::Allow));
        assert!(matches!(verdict(Mode::YoloPlan, "write", &bash, true), Verdict::Deny(_)));
        assert!(matches!(verdict(Mode::Plan, "mcp__winfer__bg", &json!({ "action": "list" }), false), Verdict::Allow));
        assert!(matches!(verdict(Mode::Plan, "mcp__winfer__bg", &json!({ "action": "start", "cmd": "make" }), false), Verdict::Ask));
        assert_eq!(allow_key("bash", &bash), "bash:cargo");
    }

    #[test]
    fn reads_the_model_list() {
        let m = json!({ "id": "gpt-6.1-sol", "name": "GPT-6.1 Sol", "provider": "openai" });
        assert_eq!(model_info(&m), Some(ModelInfo { spec: "pi/openai/gpt-6.1-sol".into(), label: "GPT-6.1 Sol".into(), detail: "Pi · openai".into(), efforts: None }));
        assert_eq!(display_name("mcp__winfer__bg"), "winfer · bg");
        assert_eq!(display_name("bash"), "Bash");
    }
}
