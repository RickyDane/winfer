//! Claude Code bridge: `claude -p` in stream-json mode, with Claude Code's own tools. Permission
//! prompts come back as `can_use_tool` control requests (`--permission-prompt-tool stdio`) and
//! are answered by the TUI.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::diff;
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Todo, Usage, UserMsg};

pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("claude").is_none() {
        return Vec::new();
    }
    [
        ("fable", "Fable", "most capable"),
        ("opus", "Opus", "complex work"),
        ("sonnet", "Sonnet", "everyday coding"),
        ("haiku", "Haiku", "fast"),
    ]
    .iter()
    .map(|(id, label, detail)| ModelInfo {
        spec: format!("claude/{id}"),
        label: format!("Claude {label}"),
        detail: format!("Claude Code · {detail}"),
    })
    .collect()
}

fn permission_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Ask => "default",
        Mode::AutoEdit => "acceptEdits",
        Mode::Plan => "plan",
        Mode::Yolo => "bypassPermissions",
    }
}

/// Claude Code's tools in lean mode.
const LEAN_TOOLS: &str = "Bash,Read,Edit,Write,Grep,Glob,TodoWrite";

pub fn args(opts: &Opts) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--allow-dangerously-skip-permissions",
        "--permission-mode",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(permission_mode(opts.mode).into());
    if !opts.model.is_empty() {
        args.extend(["--model".into(), opts.model.clone()]);
    }
    if let Some(effort) = &opts.effort {
        args.extend(["--effort".into(), effort.clone()]);
    }
    if let Some(session) = &opts.resume {
        args.extend(["--resume".into(), session.clone()]);
    }
    if opts.lean {
        // Only the tools a coding task needs, no user MCP servers, skills or user-level settings
        // (hooks, plugins), and a short system prompt: about 5k tokens per request instead of 26k.
        // winfer's own MCP servers still come in through --mcp-config; project CLAUDE.md still loads.
        args.extend([
            "--tools".into(),
            LEAN_TOOLS.into(),
            "--strict-mcp-config".into(),
            "--disable-slash-commands".into(),
            "--setting-sources".into(),
            "project,local".into(),
            "--system-prompt".into(),
            format!("{}\n\n{}", super::LEAN_PROMPT, super::environment(&opts.cwd)),
        ]);
    }
    if let Some(system) = &opts.system {
        args.extend(["--append-system-prompt".into(), system.clone()]);
    }
    if !opts.mcp.is_empty() {
        args.extend(["--mcp-config".into(), json!({ "mcpServers": opts.mcp }).to_string()]);
    }
    args
}

/// What we remember about a tool call between its start and its result.
struct Call {
    name: String,
    input: Value,
}

struct Bridge {
    events: Events,
    calls: HashMap<String, Call>,
    /// Pending permission requests: request id → (input, suggested rules).
    pending: HashMap<String, (Value, Value)>,
    /// Ids and kinds ("text", "thinking", "tool_use") of the streamed message's blocks, by index.
    blocks: HashMap<u64, (String, String)>,
    /// Streamed text blocks already matched to their final text.
    finalized: std::collections::HashSet<String>,
    message: String,
    busy: bool,
    seq: u64,
    /// Checkpoints for /undo, newest last: the uuid of each user message whose turn edited files.
    /// Claude Code snapshots files before each user message; rewinding to one undoes its turn.
    checkpoints: Vec<String>,
    /// Whether the current turn has run an edit tool (only such turns get a checkpoint).
    edited: bool,
    /// Told to Claude with the next message after an undo, so it doesn't trust stale file state.
    note: Option<String>,
}

/// Tools whose changes Claude Code's file checkpoints cover.
const EDIT_TOOLS: [&str; 4] = ["Edit", "MultiEdit", "Write", "NotebookEdit"];

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut proc = match LineProcess::spawn("claude", &args(&opts), &[("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "true")], &opts.cwd, me) {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(Ev::Exited(e));
            return;
        }
    };
    let _ = proc.write(&json!({ "type": "control_request", "request_id": "init", "request": { "subtype": "initialize" } }));
    let mut b = Bridge {
        events,
        calls: HashMap::new(),
        pending: HashMap::new(),
        blocks: HashMap::new(),
        finalized: Default::default(),
        message: String::new(),
        busy: false,
        seq: 0,
        checkpoints: Vec::new(),
        edited: false,
        note: None,
    };
    while let Some(msg) = super::recv(&rx) {
        match msg {
            In::Line(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    b.on_line(&v);
                }
            }
            In::Closed => {
                let reason = proc.exit_reason();
                let _ = b.events.send(Ev::Exited(format!("Claude Code stopped: {reason}")));
                return;
            }
            In::Cmd(cmd) => {
                if !b.on_cmd(cmd, &mut proc) {
                    return;
                }
            }
        }
    }
}

impl Bridge {
    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn control(&mut self, proc: &mut LineProcess, request: Value) {
        self.seq += 1;
        let _ = proc.write(&json!({ "type": "control_request", "request_id": format!("w{}", self.seq), "request": request }));
    }

    fn on_cmd(&mut self, cmd: Cmd, proc: &mut LineProcess) -> bool {
        match cmd {
            Cmd::Send(mut msg) => {
                self.busy = true;
                self.emit(Ev::TurnStart);
                if let Some(note) = self.note.take() {
                    msg.text = format!("{note}\n\n{}", msg.text);
                }
                let uuid = new_uuid();
                self.checkpoints.push(uuid.clone());
                self.edited = false;
                let mut line = user_message(&msg);
                line["uuid"] = json!(uuid);
                if let Err(e) = proc.write(&line) {
                    self.emit(Ev::Error(format!("couldn't reach Claude Code: {e}")));
                }
            }
            Cmd::Interrupt => self.control(proc, json!({ "subtype": "interrupt" })),
            Cmd::Approve(id, decision) => {
                let Some((input, suggestions)) = self.pending.remove(&id) else { return true };
                let response = match decision {
                    Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
                    Decision::Always => json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": suggestions }),
                    Decision::Deny => json!({ "behavior": "deny", "message": "The user declined this. Ask them how to proceed." }),
                };
                let _ = proc.write(&json!({
                    "type": "control_response",
                    "response": { "subtype": "success", "request_id": id, "response": response },
                }));
            }
            Cmd::SetModel(model) => self.control(proc, json!({ "subtype": "set_model", "model": model })),
            Cmd::SetMode(mode) => self.control(proc, json!({ "subtype": "set_permission_mode", "mode": permission_mode(mode) })),
            Cmd::SetEffort(_) => self.emit(Ev::Notice("Effort applies to the next Claude Code session (/new).".into())),
            Cmd::Compact => {
                self.busy = true;
                self.emit(Ev::TurnStart);
                let _ = proc.write(&user_message(&UserMsg { text: "/compact".into(), images: Vec::new() }));
            }
            Cmd::SetCompaction { .. } => {}
            Cmd::Undo => {
                if self.busy {
                    self.emit(Ev::Notice("Wait for the turn to finish (or press Esc) before /undo.".into()));
                } else if let Some(uuid) = self.checkpoints.pop() {
                    self.seq += 1;
                    let request = json!({ "subtype": "rewind_files", "user_message_id": uuid });
                    let _ = proc.write(&json!({ "type": "control_request", "request_id": format!("undo{}", self.seq), "request": request }));
                } else {
                    self.emit(Ev::Notice("Nothing to undo.".into()));
                }
            }
            Cmd::Shutdown => return false,
        }
        true
    }

    fn on_line(&mut self, v: &Value) {
        // Subagent traffic shows up as progress on the Task call that started it.
        if let Some(parent) = v["parent_tool_use_id"].as_str() {
            if v["type"] == "assistant" {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use" {
                        let name = block["name"].as_str().unwrap_or("tool");
                        let chunk = format!("{} {}\n", name, title(name, &block["input"]));
                        self.emit(Ev::ToolOutput { id: parent.to_string(), chunk });
                    }
                }
            }
            return;
        }
        match v["type"].as_str().unwrap_or("") {
            "system" => match v["subtype"].as_str() {
                Some("init") => {
                    if let Some(s) = v["session_id"].as_str() {
                        self.emit(Ev::Session(s.to_string()));
                    }
                    if let Some(m) = v["model"].as_str() {
                        self.emit(Ev::Model(m.to_string()));
                    }
                }
                Some("compact_boundary") => self.emit(Ev::Notice("Context compacted.".into())),
                _ => {}
            },
            "stream_event" => self.on_stream(&v["event"]),
            "assistant" => {
                let id = v["message"]["id"].as_str().unwrap_or("").to_string();
                for (i, block) in v["message"]["content"].as_array().into_iter().flatten().enumerate() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or("").to_string();
                            // The whole text replaces the streamed deltas of the same block. Claude
                            // Code sends one block per assistant event, so match by order.
                            let mut streamed: Vec<(&u64, &(String, String))> =
                                self.blocks.iter().filter(|(_, (k, kind))| kind == "text" && k.starts_with(&format!("{id}:")) && !self.finalized.contains(k)).collect();
                            streamed.sort_by_key(|(i, _)| **i);
                            let key = streamed.first().map(|(_, (k, _))| k.clone()).unwrap_or_else(|| format!("{id}:t{i}"));
                            self.finalized.insert(key.clone());
                            self.emit(Ev::Text { id: key, text });
                        }
                        Some("tool_use") => {
                            let tool_id = block["id"].as_str().unwrap_or("").to_string();
                            let name = block["name"].as_str().unwrap_or("tool").to_string();
                            let input = block["input"].clone();
                            if name == "TodoWrite" {
                                if let Ok(todos) = serde_json::from_value::<Vec<Todo>>(input["todos"].clone()) {
                                    self.emit(Ev::Todos(todos));
                                }
                            }
                            self.edited |= EDIT_TOOLS.contains(&name.as_str());
                            self.emit(Ev::ToolStart { id: tool_id.clone(), name: display_name(&name), title: title(&name, &input), input: input.clone() });
                            self.calls.insert(tool_id, Call { name, input });
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let id = block["tool_use_id"].as_str().unwrap_or("").to_string();
                    let ok = !block["is_error"].as_bool().unwrap_or(false);
                    let output = content_text(&block["content"]);
                    let diff = self.calls.remove(&id).and_then(|c| ok.then(|| edit_diff(&c.name, &c.input)).flatten());
                    self.emit(Ev::ToolEnd { id, ok, output, diff });
                }
            }
            "result" => {
                let usage = &v["usage"];
                let n = |k: &str| usage[k].as_u64().unwrap_or(0);
                let last = usage["iterations"].as_array().and_then(|i| i.last()).unwrap_or(usage);
                let ln = |k: &str| last[k].as_u64().unwrap_or(0);
                let window = v["modelUsage"]
                    .as_object()
                    .and_then(|m| m.values().filter_map(|u| u["contextWindow"].as_u64()).max())
                    .unwrap_or(200_000);
                self.emit(Ev::Usage(Usage {
                    context: ln("input_tokens") + ln("cache_read_input_tokens") + ln("cache_creation_input_tokens") + ln("output_tokens"),
                    window,
                    input: n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"),
                    output: n("output_tokens"),
                    cached: n("cache_read_input_tokens"),
                    cost: v["total_cost_usd"].as_f64(),
                }));
                if v["is_error"].as_bool() == Some(true) && v["subtype"] != "error_during_execution" {
                    let text = v["result"].as_str().unwrap_or("Claude Code reported an error.");
                    self.emit(Ev::Error(text.to_string()));
                }
                self.busy = false;
                if !self.edited && !self.checkpoints.is_empty() {
                    self.checkpoints.pop();
                }
                self.edited = true; // a later result without a new message (e.g. /compact) keeps the stack
                self.blocks.clear();
                self.finalized.clear();
                self.emit(Ev::TurnDone);
            }
            "control_response" => {
                let r = &v["response"];
                if r["request_id"].as_str().is_some_and(|id| id.starts_with("undo")) {
                    self.on_undo(r);
                }
            }
            "control_request" => {
                let req = &v["request"];
                let id = v["request_id"].as_str().unwrap_or("").to_string();
                if req["subtype"] == "can_use_tool" {
                    let name = req["tool_name"].as_str().unwrap_or("tool");
                    let input = req["input"].clone();
                    let detail = req["description"]
                        .as_str()
                        .or_else(|| req["decision_reason"].as_str())
                        .unwrap_or("")
                        .to_string();
                    self.pending.insert(id.clone(), (input.clone(), req["permission_suggestions"].clone()));
                    self.emit(Ev::Approval(Approval {
                        id,
                        tool: display_name(name),
                        title: title(name, &input),
                        detail,
                        diff: edit_diff(name, &input),
                    }));
                }
            }
            _ => {}
        }
    }

    fn on_undo(&mut self, r: &Value) {
        let res = &r["response"];
        let error = r["error"].as_str().or_else(|| res["error"].as_str());
        if r["subtype"] != "success" || res["canRewind"] == false || error.is_some() {
            self.emit(Ev::Notice(format!("Couldn't undo: {}", error.unwrap_or("Claude Code has no checkpoint for that turn"))));
            return;
        }
        let mut names: Vec<String> = res["filesChanged"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str())
            .map(crate::harness::tools::short_path)
            .collect();
        names.sort();
        let list = if names.is_empty() { "the last turn's file changes".to_string() } else { names.join(", ") };
        self.note = Some(format!("(The user reverted your file changes from the last turn: {list}. Re-read files before editing them.)"));
        self.emit(Ev::Notice(format!("Reverted {list}")));
    }

    fn on_stream(&mut self, e: &Value) {
        match e["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.message = e["message"]["id"].as_str().unwrap_or("").to_string();
            }
            "content_block_start" => {
                let index = e["index"].as_u64().unwrap_or(0);
                let kind = e["content_block"]["type"].as_str().unwrap_or("");
                self.blocks.insert(index, (format!("{}:{index}", self.message), kind.to_string()));
            }
            "content_block_delta" => {
                let index = e["index"].as_u64().unwrap_or(0);
                let Some((id, _)) = self.blocks.get(&index) else { return };
                let id = id.clone();
                let d = &e["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => self.emit(Ev::TextDelta { id, text: d["text"].as_str().unwrap_or("").into() }),
                    Some("thinking_delta") => self.emit(Ev::ReasoningDelta { id, text: d["thinking"].as_str().unwrap_or("").into() }),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn user_message(msg: &UserMsg) -> Value {
    let mut content = vec![json!({ "type": "text", "text": msg.text })];
    for path in &msg.images {
        if let Some((media, data)) = crate::harness::tools::image_base64(path) {
            content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": data } }));
        }
    }
    json!({ "type": "user", "message": { "role": "user", "content": content } })
}

/// A random v4 UUID, the format Claude Code expects for message ids.
fn new_uuid() -> String {
    let n = (rand::random::<u128>() & !(0xf << 76) & !(0x3 << 62)) | (0x4 << 76) | (0x2 << 62);
    let h = format!("{n:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn display_name(name: &str) -> String {
    match name.strip_prefix("mcp__") {
        Some(rest) => rest.replacen("__", " · ", 1),
        None => name.to_string(),
    }
}

/// A one-line summary of a tool call for its card.
pub fn title(name: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let t = match name {
        "Bash" => s("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => crate::harness::tools::short_path(&s("file_path")),
        "Grep" => format!("{} {}", s("pattern"), s("path")),
        "Glob" => s("pattern"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "Task" | "Agent" => s("description"),
        "TodoWrite" => "update todos".into(),
        _ => input
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string)))
            .unwrap_or_default(),
    };
    t.lines().next().unwrap_or("").to_string()
}

/// The diff an edit tool makes, from its input.
fn edit_diff(name: &str, input: &Value) -> Option<String> {
    let path = input["file_path"].as_str()?;
    let short = crate::harness::tools::short_path(path);
    match name {
        "Edit" => Some(diff::unified(&short, input["old_string"].as_str()?, input["new_string"].as_str()?)),
        "MultiEdit" => {
            let mut out = String::new();
            for e in input["edits"].as_array()? {
                out.push_str(&diff::unified(&short, e["old_string"].as_str()?, e["new_string"].as_str()?));
            }
            Some(out)
        }
        "Write" => {
            let old = std::fs::read_to_string(path).unwrap_or_default();
            Some(diff::unified(&short, &old, input["content"].as_str()?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn bridge() -> (Bridge, std::sync::mpsc::Receiver<Ev>) {
        let (tx, rx) = channel();
        let b = Bridge {
            events: tx,
            calls: HashMap::new(),
            pending: HashMap::new(),
            blocks: HashMap::new(),
            finalized: Default::default(),
            message: String::new(),
            busy: true,
            seq: 0,
            checkpoints: Vec::new(),
            edited: false,
            note: None,
        };
        (b, rx)
    }

    #[test]
    fn lean_mode_trims_the_setup() {
        let opts = |lean| Opts {
            cwd: std::env::temp_dir(),
            model: String::new(),
            effort: None,
            mode: Mode::Ask,
            resume: None,
            system: None,
            mcp: Default::default(),
            allowed_commands: Vec::new(),
            trim_at: 60,
            compact_at: 85,
            lean,
        };
        let lean = args(&opts(true));
        assert!(lean.windows(2).any(|w| w[0] == "--tools" && w[1] == LEAN_TOOLS));
        assert!(lean.iter().any(|a| a == "--strict-mcp-config") && lean.iter().any(|a| a == "--system-prompt"));
        let full = args(&opts(false));
        assert!(!full.iter().any(|a| a == "--tools" || a == "--system-prompt"));
    }

    #[test]
    fn maps_a_turn_with_a_tool_call() {
        let (mut b, rx) = bridge();
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-opus"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Hi!"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/x/a.rs","old_string":"a\n","new_string":"b\n"}}]}}"#,
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":[]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"result","is_error":false,"total_cost_usd":0.5,"usage":{"input_tokens":3,"output_tokens":4,"cache_read_input_tokens":10},"modelUsage":{"x":{"contextWindow":1000}}}"#,
        ];
        for l in lines {
            b.on_line(&serde_json::from_str(l).unwrap());
        }
        let evs: Vec<Ev> = rx.try_iter().collect();
        assert!(matches!(&evs[0], Ev::Session(s) if s == "s1"));
        assert!(evs.iter().any(|e| matches!(e, Ev::TextDelta { id, text } if id == "m1:1" && text == "Hi")));
        assert!(evs.iter().any(|e| matches!(e, Ev::Text { id, text } if id == "m1:1" && text == "Hi!")));
        assert!(evs.iter().any(|e| matches!(e, Ev::ToolStart { name, title, .. } if name == "Edit" && title.ends_with("a.rs"))));
        assert!(evs.iter().any(|e| matches!(e, Ev::Approval(a) if a.id == "r1" && a.title == "ls")));
        assert!(evs.iter().any(|e| matches!(e, Ev::ToolEnd { ok: true, diff: Some(d), .. } if d.contains("+b"))));
        assert!(evs.iter().any(|e| matches!(e, Ev::Usage(u) if u.window == 1000 && u.cost == Some(0.5))));
        assert!(matches!(evs.last(), Some(Ev::TurnDone)));
    }

    #[test]
    fn undo_keeps_only_turns_that_edited() {
        let (mut b, rx) = bridge();
        let result = r#"{"type":"result","is_error":false,"usage":{}}"#;
        b.checkpoints.push("u1".into());
        b.on_line(&serde_json::from_str(r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"Write","input":{"file_path":"/x/a","content":""}}]}}"#).unwrap());
        b.on_line(&serde_json::from_str(result).unwrap());
        b.checkpoints.push("u2".into());
        b.edited = false;
        b.on_line(&serde_json::from_str(result).unwrap());
        assert_eq!(b.checkpoints, ["u1"]);
        b.on_line(&serde_json::from_str(r#"{"type":"control_response","response":{"subtype":"success","request_id":"undo1","response":{"canRewind":true,"filesChanged":["/x/a"]}}}"#).unwrap());
        assert!(rx.try_iter().any(|e| matches!(e, Ev::Notice(n) if n == "Reverted /x/a")));
        assert!(b.note.as_deref().is_some_and(|n| n.contains("/x/a")));
        let id = new_uuid();
        assert_eq!((id.len(), &id[14..15]), (36, "4"));
    }
}
