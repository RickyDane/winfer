//! Claude Code bridge: `claude -p` in stream-json mode, with Claude Code's own tools. Permission
//! prompts come back as `can_use_tool` control requests (`--permission-prompt-tool stdio`) and
//! are answered by the TUI.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::claude_skills::{self, LeanSkills};
use crate::harness::diff;
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Question, Skill, TaskEv, Todo, TurnTokens, Usage, UserMsg};

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
        Mode::Plan | Mode::YoloPlan => "plan",
        Mode::Yolo => "bypassPermissions",
    }
}

fn mode_of(mode: &str) -> Option<Mode> {
    Some(match mode {
        "default" => Mode::Ask,
        "acceptEdits" => Mode::AutoEdit,
        "plan" => Mode::Plan,
        "bypassPermissions" => Mode::Yolo,
        _ => return None,
    })
}

/// Claude Code's tools in lean mode; `Skill` comes on top when it has skills to load.
/// TodoWrite is the plan tool of older Claude Code versions; newer ones plan with the Task* tools.
/// AskUserQuestion's questions come to the TUI as an approval with `questions`.
/// No ToolSearch: MCP tools come with their full definitions up front, as an MCP server's
/// tools/list gives them, instead of only their names to be looked up first (see `run`'s
/// ENABLE_TOOL_SEARCH, which covers the non-lean mode too).
/// ExitPlanMode is how plan mode hands its plan over for approval; without it the agent can't leave plan mode.
const LEAN_TOOLS: &str = "Bash,Read,Edit,Write,Grep,Glob,TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet,Agent,AskUserQuestion,ExitPlanMode";

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
        // Only the tools a coding task needs, no user MCP servers or user-level settings (hooks,
        // plugins) unless all skills are wanted, and a short system prompt: about 5k tokens per
        // request instead of 26k. winfer's own MCP servers still come in through --mcp-config;
        // project CLAUDE.md and project skills still load.
        let picked = match &opts.skills {
            LeanSkills::All => None,
            LeanSkills::Only(ids) if ids.is_empty() => Some(None),
            LeanSkills::Only(ids) => Some(claude_skills::prepare(ids)),
        };
        let skills = !matches!(picked, Some(None));
        args.extend([
            "--tools".into(),
            if skills { format!("{LEAN_TOOLS},Skill") } else { LEAN_TOOLS.into() },
            "--strict-mcp-config".into(),
            "--setting-sources".into(),
            if picked.is_none() { "user,project,local" } else { "project,local" }.into(),
        ]);
        match picked {
            Some(None) => args.push("--disable-slash-commands".into()),
            Some(Some(dir)) => args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]),
            None => {}
        }
        let prompt = if opts.mcp.contains_key("codegraph") { super::LEAN_CODEGRAPH_PROMPT } else { super::LEAN_PROMPT };
        args.extend(["--system-prompt".into(), format!("{prompt}\n\n{}", super::environment(&opts.cwd))]);
    }
    if let Some(system) = &opts.system {
        args.extend(["--append-system-prompt".into(), system.clone()]);
    }
    if !opts.mcp.is_empty() {
        args.extend(["--mcp-config".into(), json!({ "mcpServers": opts.mcp }).to_string()]);
    }
    args
}

/// AskUserQuestion's questions.
fn questions(input: &Value) -> Vec<Question> {
    let text = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| Question {
            header: text(&q["header"]),
            question: text(&q["question"]),
            options: q["options"].as_array().into_iter().flatten().map(|o| (text(&o["label"]), text(&o["description"]))).collect(),
            multi: q["multiSelect"].as_bool().unwrap_or(false),
        })
        .filter(|q| !q.question.is_empty())
        .collect()
}

/// AskUserQuestion's input with the user's answers, which it takes as `answers`: question → answer.
fn answered(mut input: Value, answers: Vec<String>) -> Value {
    let map: serde_json::Map<String, Value> = input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(answers)
        .map(|(q, a)| (q["question"].as_str().unwrap_or("").to_string(), json!(a)))
        .collect();
    input["answers"] = Value::Object(map);
    input
}

/// What we remember about a tool call between its start and its result.
struct Call {
    name: String,
    input: Value,
}

/// Usage as the turn runs. Claude Code's `result` gives the turn's totals; until it arrives the
/// turn is added up from stream events: each request's input at `message_start`, its exact
/// output at `message_delta`, and the text streamed in between as an estimate.
#[derive(Default)]
struct Live {
    /// The last usage sent: session totals, the window and the session's cost.
    usage: Usage,
    /// Totals of the session's finished turns.
    input: u64,
    cached: u64,
    output: u64,
    /// The running turn: its input so far, its finished requests' output, and the streaming
    /// request's output.
    turn_input: u64,
    turn_cached: u64,
    done: u64,
    msg: u64,
    turn: TurnTokens,
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
    /// The running turn's usage, built from stream events so the token count moves while the
    /// agent works instead of only when the turn's `result` arrives.
    live: Live,
    busy: bool,
    seq: u64,
    /// The user turns for /undo, newest last. Claude Code snapshots files before each user
    /// message and can rewind its conversation to one, both by the message's uuid.
    checkpoints: Vec<Checkpoint>,
    /// An undo waiting on Claude Code: the files it reverted so far.
    undoing: Option<Vec<String>>,
    /// Lines to write to Claude Code once the current one is handled.
    outbox: Vec<Value>,
    plan: Plan,
    /// The permission mode as winfer sees it (Claude Code only knows "plan" for yolo-plan).
    mode: Mode,
}

struct Checkpoint {
    /// The uuid given to the user's message.
    uuid: String,
    /// Whether the turn ran an edit tool (else there are no file changes to rewind).
    edited: bool,
}

/// The plan Claude Code keeps with its TaskCreate/TaskUpdate tools, mirrored as todos.
#[derive(Default)]
struct Plan {
    tasks: Vec<(String, Todo)>,
}

impl Plan {
    /// A TaskCreate result ("Task #3 created successfully: …") adds the task.
    fn created(&mut self, input: &Value, result: &str) -> bool {
        let Some(id) = result.split_once("Task #").and_then(|(_, r)| r.split(|c: char| !c.is_ascii_digit()).next()).filter(|id| !id.is_empty()) else {
            return false;
        };
        let content = input["subject"].as_str().or_else(|| input["description"].as_str()).unwrap_or("").trim().to_string();
        // Claude Code keeps tasks for the whole session; a new plan after a finished one starts clean.
        if self.tasks.iter().all(|(_, t)| t.status == "completed") {
            self.tasks.clear();
        }
        self.tasks.retain(|(i, _)| i != id);
        self.tasks.push((id.to_string(), Todo { content, status: "pending".into() }));
        true
    }

    /// A TaskUpdate call: new status ("deleted" removes it) or subject.
    fn update(&mut self, input: &Value) -> bool {
        let id = match &input["taskId"] {
            Value::String(s) => s.trim_start_matches('#').to_string(),
            Value::Number(n) => n.to_string(),
            _ => return false,
        };
        let Some(i) = self.tasks.iter().position(|(t, _)| *t == id) else { return false };
        if input["status"] == "deleted" {
            self.tasks.remove(i);
            return true;
        }
        let todo = &mut self.tasks[i].1;
        if let Some(st) = input["status"].as_str().filter(|s| matches!(*s, "pending" | "in_progress" | "completed")) {
            todo.status = st.to_string();
        }
        if let Some(subject) = input["subject"].as_str().filter(|s| !s.trim().is_empty()) {
            todo.content = subject.trim().to_string();
        }
        true
    }

    fn todos(&self) -> Vec<Todo> {
        self.tasks.iter().map(|(_, t)| t.clone()).collect()
    }
}

/// Claude Code's request to drop a user message and everything after it from the conversation.
fn rewind_conversation(uuid: &str) -> Value {
    json!({ "subtype": "rewind_conversation", "target_message_uuid": uuid, "interrupt_if_running": false })
}

/// Tools whose changes Claude Code's file checkpoints cover.
const EDIT_TOOLS: [&str; 4] = ["Edit", "MultiEdit", "Write", "NotebookEdit"];

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut proc = match LineProcess::spawn("claude", &args(&opts), &[
        ("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "true"),
        // Headless Claude Code has no plan tool unless asked for one (TaskCreate/TaskUpdate).
        ("CLAUDE_CODE_ENABLE_TODO_TOOLS", "true"),
        // MCP tools load directly with their descriptions, not deferred behind ToolSearch.
        ("ENABLE_TOOL_SEARCH", "false"),
    ], &opts.cwd, me) {
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
        live: Live::default(),
        busy: false,
        seq: 0,
        checkpoints: Vec::new(),
        undoing: None,
        outbox: Vec::new(),
        plan: Plan::default(),
        mode: opts.mode,
    };
    while let Some(msg) = super::recv(&rx) {
        match msg {
            In::Line(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    b.on_line(&v);
                }
                for out in std::mem::take(&mut b.outbox) {
                    let _ = proc.write(&out);
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
    /// Sends the running turn's usage on top of the session's finished turns.
    fn emit_live(&mut self) {
        let l = &mut self.live;
        let turn = l.turn.now();
        l.usage.input = l.input + l.turn_input;
        l.usage.cached = l.cached + l.turn_cached;
        l.usage.output = l.output + turn;
        l.usage.turn = turn;
        l.usage.turn_input = l.turn_input;
        l.usage.turn_cached = l.turn_cached;
        if l.usage.window == 0 {
            l.usage.window = 200_000;
        }
        let u = l.usage.clone();
        self.emit(Ev::Usage(u));
    }

    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn control(&mut self, proc: &mut LineProcess, request: Value) {
        self.seq += 1;
        let _ = proc.write(&json!({ "type": "control_request", "request_id": format!("w{}", self.seq), "request": request }));
    }

    fn on_cmd(&mut self, cmd: Cmd, proc: &mut LineProcess) -> bool {
        match cmd {
            Cmd::Send(msg) => {
                self.busy = true;
                self.emit(Ev::TurnStart);
                let uuid = new_uuid();
                self.checkpoints.push(Checkpoint { uuid: uuid.clone(), edited: false });
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
            Cmd::Answer(id, answers) => {
                let Some((input, _)) = self.pending.remove(&id) else { return true };
                let input = answered(input, answers);
                let _ = proc.write(&json!({
                    "type": "control_response",
                    "response": { "subtype": "success", "request_id": id, "response": { "behavior": "allow", "updatedInput": input } },
                }));
            }
            Cmd::SetModel(model) => self.control(proc, json!({ "subtype": "set_model", "model": model })),
            Cmd::SetMode(mode) => {
                self.mode = mode;
                self.control(proc, json!({ "subtype": "set_permission_mode", "mode": permission_mode(mode) }));
            }
            // Live: Claude Code applies `effortLevel` from flag settings to the running session (as
            // its own /effort does); null goes back to the model's default.
            Cmd::SetEffort(e) => {
                let level = e.map(|e| if e == "minimal" { "low".to_string() } else { e });
                self.control(proc, json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": level } }));
            }
            Cmd::Compact => {
                self.busy = true;
                self.emit(Ev::TurnStart);
                let _ = proc.write(&user_message(&UserMsg { text: "/compact".into(), images: Vec::new(), skill: None }));
            }
            Cmd::SetCompaction { .. } => {}
            Cmd::Undo { files } => {
                if self.busy || self.undoing.is_some() {
                    self.emit(Ev::Undone(Err("Wait for the turn to finish (or press Esc) before /undo.".into())));
                } else if let Some(cp) = self.checkpoints.last() {
                    let uuid = cp.uuid.clone();
                    self.undoing = Some(Vec::new());
                    self.seq += 1;
                    // Files first (rewinding the conversation forgets the message's file snapshot).
                    let (id, request) = if files && cp.edited {
                        ("undofiles", json!({ "subtype": "rewind_files", "user_message_id": uuid }))
                    } else {
                        ("undoconv", rewind_conversation(&uuid))
                    };
                    let _ = proc.write(&json!({ "type": "control_request", "request_id": format!("{id}{}", self.seq), "request": request }));
                } else {
                    self.emit(Ev::Undone(Err("Nothing to undo.".into())));
                }
            }
            Cmd::StopTask(id) => self.control(proc, json!({ "subtype": "stop_task", "task_id": id })),
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
        // Claude Code starts a turn on its own when a background task finishes (it reads the
        // task's notification and replies); show it as working like any other turn.
        if !self.busy && matches!(v["type"].as_str(), Some("assistant" | "stream_event")) {
            self.busy = true;
            self.emit(Ev::TurnStart);
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
                    if let Some(skills) = init_skills(v) {
                        self.emit(Ev::Skills(skills));
                    }
                }
                Some("compact_boundary") => self.emit(Ev::Notice("Context compacted.".into())),
                // Claude Code changed its permission mode itself (an approved plan leaves plan mode).
                Some("status") => {
                    if let Some(m) = v["permissionMode"].as_str().and_then(mode_of) {
                        if permission_mode(m) != permission_mode(self.mode) {
                            self.mode = m;
                            self.emit(Ev::Mode(m));
                        }
                    }
                }
                Some("task_started" | "task_progress" | "task_updated" | "task_notification") => {
                    if let Some(t) = task_event(v) {
                        self.emit(Ev::Task(t));
                    }
                }
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
                            if name == "TaskUpdate" && self.plan.update(&input) {
                                self.emit(Ev::Todos(self.plan.todos()));
                            }
                            if EDIT_TOOLS.contains(&name.as_str()) {
                                if let Some(cp) = self.checkpoints.last_mut() {
                                    cp.edited = true;
                                }
                            }
                            self.emit(Ev::ToolStart { id: tool_id.clone(), name: display_name(&name), title: title(&name, &input), input: input.clone() });
                            self.calls.insert(tool_id, Call { name, input });
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                if let Some(t) = shell_notification(&content_text(&v["message"]["content"])) {
                    self.emit(Ev::Task(t));
                }
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let id = block["tool_use_id"].as_str().unwrap_or("").to_string();
                    let ok = !block["is_error"].as_bool().unwrap_or(false);
                    let output = content_text(&block["content"]);
                    let call = self.calls.remove(&id);
                    if let Some(c) = call.as_ref().filter(|c| ok && c.name == "Bash") {
                        if let Some(t) = shell_receipt(&id, &c.input, &output) {
                            self.emit(Ev::Task(t));
                        }
                    }
                    if let Some(c) = call.as_ref().filter(|c| ok && c.name == "TaskCreate") {
                        if self.plan.created(&c.input, &output) {
                            self.emit(Ev::Todos(self.plan.todos()));
                        }
                    }
                    let diff = call.and_then(|c| ok.then(|| edit_diff(&c.name, &c.input)).flatten());
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
                // `usage` is this turn's; the session's totals add up across turns.
                let l = &mut self.live;
                l.input += n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                l.cached += n("cache_read_input_tokens");
                l.output += n("output_tokens");
                let u = Usage {
                    context: ln("input_tokens") + ln("cache_read_input_tokens") + ln("cache_creation_input_tokens") + ln("output_tokens"),
                    window,
                    input: l.input,
                    output: l.output,
                    cached: l.cached,
                    cost: v["total_cost_usd"].as_f64(),
                    turn: n("output_tokens"),
                    turn_input: n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"),
                    turn_cached: n("cache_read_input_tokens"),
                };
                *l = Live { usage: u.clone(), input: l.input, cached: l.cached, output: l.output, ..Default::default() };
                self.emit(Ev::Usage(u));
                if v["is_error"].as_bool() == Some(true) && v["subtype"] != "error_during_execution" {
                    let text = v["result"].as_str().unwrap_or("Claude Code reported an error.");
                    self.emit(Ev::Error(text.to_string()));
                }
                self.busy = false;
                self.blocks.clear();
                self.finalized.clear();
                self.emit(Ev::TurnDone);
            }
            "control_response" => {
                let r = &v["response"];
                match r["request_id"].as_str() {
                    Some(id) if id.starts_with("undofiles") => self.on_rewind_files(r),
                    Some(id) if id.starts_with("undoconv") => self.on_rewind_conversation(r),
                    _ => {}
                }
            }
            "control_request" => {
                let req = &v["request"];
                let id = v["request_id"].as_str().unwrap_or("").to_string();
                if req["subtype"] == "can_use_tool" {
                    let name = req["tool_name"].as_str().unwrap_or("tool");
                    let input = req["input"].clone();
                    // Yolo-plan answers on its own: edits are refused, the rest runs. Handing
                    // over the plan and questions still go to the user.
                    if self.mode == Mode::YoloPlan && !matches!(name, "ExitPlanMode" | "AskUserQuestion") {
                        let response = if EDIT_TOOLS.contains(&name) {
                            json!({ "behavior": "deny", "message": "Plan mode is on: files can't be edited. Describe the change in the plan instead." })
                        } else {
                            json!({ "behavior": "allow", "updatedInput": input })
                        };
                        self.outbox.push(json!({ "type": "control_response", "response": { "subtype": "success", "request_id": id, "response": response } }));
                        return;
                    }
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
                        questions: if name == "AskUserQuestion" { questions(&input) } else { Vec::new() },
                    }));
                }
            }
            _ => {}
        }
    }

    /// Claude Code put back the turn's files; now drop the turn from the conversation.
    fn on_rewind_files(&mut self, r: &Value) {
        let res = &r["response"];
        let error = r["error"].as_str().or_else(|| res["error"].as_str());
        if r["subtype"] != "success" || res["canRewind"] == false || error.is_some() {
            self.undoing = None;
            let why = error.unwrap_or("Claude Code has no file checkpoint for that turn");
            self.emit(Ev::Undone(Err(format!("Couldn't revert the files: {why}"))));
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
        self.undoing = Some(names);
        let Some(cp) = self.checkpoints.last() else { return };
        self.seq += 1;
        self.outbox.push(json!({ "type": "control_request", "request_id": format!("undoconv{}", self.seq), "request": rewind_conversation(&cp.uuid) }));
    }

    fn on_rewind_conversation(&mut self, r: &Value) {
        let files = self.undoing.take().unwrap_or_default();
        let res = &r["response"];
        if r["subtype"] == "success" && res["rewound"] == true {
            self.checkpoints.pop();
            self.emit(Ev::Undone(Ok(files)));
            return;
        }
        let why = r["error"].as_str().or_else(|| res["error"].as_str()).unwrap_or("Claude Code refused");
        let reverted = if files.is_empty() { String::new() } else { format!(" (its files were reverted: {})", files.join(", ")) };
        self.emit(Ev::Undone(Err(format!("Couldn't drop the turn from the conversation: {why}{reverted}"))));
    }

    fn on_stream(&mut self, e: &Value) {
        match e["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.message = e["message"]["id"].as_str().unwrap_or("").to_string();
                let u = &e["message"]["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let input = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                let l = &mut self.live;
                l.turn_input += input;
                l.turn_cached += n("cache_read_input_tokens");
                l.msg = n("output_tokens");
                l.turn.exact(l.done + l.msg);
                l.usage.context = input + l.msg;
                self.emit_live();
            }
            "message_delta" => {
                if let Some(out) = e["usage"]["output_tokens"].as_u64() {
                    let l = &mut self.live;
                    l.usage.context += out.saturating_sub(l.msg);
                    l.msg = out;
                    l.turn.exact(l.done + l.msg);
                    self.emit_live();
                }
            }
            "message_stop" => {
                let l = &mut self.live;
                l.done = l.turn.now();
                l.msg = 0;
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
                if let Some(text) = ["text", "thinking", "partial_json"].iter().find_map(|k| d[k].as_str()) {
                    if self.live.turn.streamed(text) {
                        self.emit_live();
                    }
                }
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

/// Claude Code's `system` task events (task_started, task_progress, task_updated,
/// task_notification) as one update.
fn task_event(v: &Value) -> Option<TaskEv> {
    let id = v["task_id"].as_str().filter(|s| !s.is_empty())?.to_string();
    let s = |k: &Value| k.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let mut t = TaskEv {
        id, tool_id: s(&v["tool_use_id"]), title: s(&v["description"]), agent: s(&v["subagent_type"]),
        output_file: s(&v["output_file"]), exit_code: v["exit_code"].as_i64().and_then(|n| i32::try_from(n).ok()),
        ..Default::default()
    };
    match v["subtype"].as_str() {
        Some("task_started") => t.status = Some("running".into()),
        Some("task_progress") => {
            t.tools = v["usage"]["tool_uses"].as_u64();
            t.last = s(&v["last_tool_name"]);
        }
        Some("task_updated") => {
            t.status = s(&v["patch"]["status"]);
            t.title = t.title.or_else(|| s(&v["patch"]["description"]));
        }
        Some("task_notification") => {
            t.status = s(&v["status"]).or_else(|| Some("completed".into()));
            t.tools = v["usage"]["tool_uses"].as_u64();
        }
        _ => return None,
    }
    Some(t)
}

/// Older Claude Code releases only announce background shells in their Bash receipt.
fn shell_receipt(tool_id: &str, input: &Value, output: &str) -> Option<TaskEv> {
    let rest = output.strip_prefix("Command running in background with ID: ")?;
    let id = rest.split_once('.').map(|(id, _)| id)?.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let output_file = output.split_once("Output is being written to: ").map(|(_, path)| {
        path.split_once(". You ").map_or(path.trim_end_matches('.').trim(), |(path, _)| path).to_string()
    });
    Some(TaskEv {
        id: id.into(), tool_id: Some(tool_id.into()), title: Some(title("Bash", input)),
        status: Some("running".into()), output_file, ..Default::default()
    })
}

fn shell_notification(text: &str) -> Option<TaskEv> {
    let text = text.strip_prefix("<task-notification>")?.split_once("</task-notification>")?.0;
    let tag = |name: &str| text.split_once(&format!("<{name}>"))?.1.split_once(&format!("</{name}>"))
        .map(|(value, _)| value.trim().to_string()).filter(|s| !s.is_empty());
    let summary = tag("summary").unwrap_or_default();
    let exit_code = summary.rsplit_once("(exit code ").and_then(|(_, code)| code.trim_end_matches(')').parse().ok());
    Some(TaskEv {
        id: tag("task-id")?, tool_id: tag("tool-use-id"), status: tag("status"),
        output_file: tag("output-file"), exit_code, ..Default::default()
    })
}

/// The skills a session loaded (`skills` in its init message), with descriptions from disk.
fn init_skills(v: &Value) -> Option<Vec<Skill>> {
    let names = v["skills"].as_array()?;
    let known = crate::harness::claude_skills::available();
    Some(
        names
            .iter()
            .filter_map(|n| n.as_str())
            .map(|name| Skill {
                name: name.to_string(),
                description: known.iter().find(|k| k.id == name || k.name == name).map(|k| k.description.clone()).unwrap_or_default(),
            })
            .collect(),
    )
}

fn user_message(msg: &UserMsg) -> Value {
    // Claude Code runs `/name args` typed into a turn as that skill.
    let text = match &msg.skill {
        Some(skill) => format!("/{skill} {}", msg.text).trim_end().to_string(),
        None => msg.text.clone(),
    };
    let mut content = vec![json!({ "type": "text", "text": text })];
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
        "Skill" => format!("{} {}", s("skill"), s("args")).trim_end().to_string(),
        "TodoWrite" => "update todos".into(),
        "ExitPlanMode" => "approve the plan and start working".into(),
        "AskUserQuestion" => input["questions"][0]["question"].as_str().unwrap_or("").to_string(),
        "TaskCreate" => s("subject"),
        "TaskUpdate" => {
            let id = input["taskId"].as_str().map(str::to_string).or_else(|| input["taskId"].as_u64().map(|n| n.to_string())).unwrap_or_default();
            let what = if input["status"].is_string() { s("status").replace('_', " ") } else { "updated".into() };
            format!("#{id} {what}")
        }
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
    #[test]
    fn skill_cards_show_the_skill_name() {
        use serde_json::json;
        assert_eq!(super::title("Skill", &json!({ "args": "ultra", "skill": "caveman" })), "caveman ultra");
        assert_eq!(super::title("Skill", &json!({ "skill": "graphify" })), "graphify");
    }

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
            live: Live::default(),
            busy: true,
            seq: 0,
            checkpoints: Vec::new(),
            undoing: None,
            outbox: Vec::new(),
            plan: Plan::default(),
            mode: Mode::Ask,
        };
        (b, rx)
    }

    #[test]
    fn lean_mode_trims_the_setup() {
        let opts = |lean, skills| Opts {
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
            skills,
        };
        let has = |args: &[String], flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        let lean = args(&opts(true, LeanSkills::Only(Vec::new())));
        assert!(has(&lean, "--tools", LEAN_TOOLS) && has(&lean, "--setting-sources", "project,local"));
        assert!(lean.iter().any(|a| a == "--strict-mcp-config") && lean.iter().any(|a| a == "--system-prompt"));
        assert!(lean.iter().any(|a| a == "--disable-slash-commands"));
        // All skills: the user's settings load, and the Skill tool comes with them.
        let all = args(&opts(true, LeanSkills::All));
        assert!(has(&all, "--tools", &format!("{LEAN_TOOLS},Skill")) && has(&all, "--setting-sources", "user,project,local"));
        assert!(!all.iter().any(|a| a == "--disable-slash-commands" || a == "--add-dir"));
        let full = args(&opts(false, LeanSkills::All));
        assert!(!full.iter().any(|a| a == "--tools" || a == "--system-prompt"));
        // With codegraph attached, the prompt explores through it rather than "search and reads".
        assert!(lean.iter().any(|a| a.starts_with(super::super::LEAN_PROMPT)));
        let mut cg = opts(true, LeanSkills::Only(Vec::new()));
        cg.mcp.insert("codegraph".into(), json!({ "command": "codegraph" }));
        assert!(args(&cg).iter().any(|a| a.starts_with(super::super::LEAN_CODEGRAPH_PROMPT)));
    }

    #[test]
    fn asks_the_users_questions_and_hands_back_their_answers() {
        let (mut b, rx) = bridge();
        let input = json!({ "questions": [
            { "header": "Lib", "question": "Which library?", "multiSelect": false, "options": [{ "label": "serde", "description": "derive" }, { "label": "nom" }] },
            { "header": "Targets", "question": "Which targets?", "multiSelect": true, "options": [{ "label": "mac" }, { "label": "linux" }] },
        ]});
        b.on_line(&json!({ "type": "control_request", "request_id": "r1", "request": { "subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": input } }));
        let a = rx.try_iter().find_map(|e| if let Ev::Approval(a) = e { Some(a) } else { None }).unwrap();
        assert_eq!(a.title, "Which library?");
        assert_eq!(a.questions.len(), 2);
        assert_eq!(a.questions[0].options[0], ("serde".to_string(), "derive".to_string()));
        assert!(!a.questions[0].multi && a.questions[1].multi);
        let out = answered(input, vec!["nom".into(), "mac, linux".into()]);
        assert_eq!(out["answers"], json!({ "Which library?": "nom", "Which targets?": "mac, linux" }));
        assert!(out["questions"].is_array());
    }

    #[test]
    fn follows_claude_codes_own_mode_changes() {
        assert!(LEAN_TOOLS.split(',').any(|t| t == "ExitPlanMode"));
        assert!(!LEAN_TOOLS.split(',').any(|t| t == "ToolSearch"));
        let (mut b, rx) = bridge();
        b.mode = Mode::Plan;
        b.on_line(&json!({ "type": "system", "subtype": "status", "status": null, "permissionMode": "default" }));
        assert!(rx.try_iter().any(|e| matches!(e, Ev::Mode(Mode::Ask))));
    }

    #[test]
    fn yolo_plan_answers_permission_requests_itself() {
        let (mut b, rx) = bridge();
        b.mode = Mode::YoloPlan;
        let ask = |b: &mut Bridge, id: &str, tool: &str| {
            b.on_line(&json!({ "type": "control_request", "request_id": id, "request": { "subtype": "can_use_tool", "tool_name": tool, "input": {} } }));
            b.outbox.pop().map(|o| o["response"]["response"]["behavior"].as_str().unwrap().to_string())
        };
        assert_eq!(ask(&mut b, "r1", "mcp__winfer-browser__browser_navigate").as_deref(), Some("allow"));
        assert_eq!(ask(&mut b, "r2", "Edit").as_deref(), Some("deny"));
        assert_eq!(ask(&mut b, "r3", "ExitPlanMode"), None);
        let approvals: Vec<String> = rx.try_iter().filter_map(|e| if let Ev::Approval(a) = e { Some(a.tool) } else { None }).collect();
        assert_eq!(approvals, ["ExitPlanMode"]);
        // Claude Code reports yolo-plan as plain "plan": not a mode change.
        b.on_line(&json!({ "type": "system", "subtype": "status", "permissionMode": "plan" }));
        assert_eq!(b.mode, Mode::YoloPlan);
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
    fn output_tokens_count_up_during_the_turn() {
        let (mut b, rx) = bridge();
        let ev = |e: Value| json!({ "type": "stream_event", "event": e });
        let outputs = |rx: &std::sync::mpsc::Receiver<Ev>| rx.try_iter().filter_map(|e| if let Ev::Usage(u) = e { Some(u.turn) } else { None }).collect::<Vec<_>>();
        b.on_line(&ev(json!({ "type": "message_start", "message": { "id": "m1", "usage": { "input_tokens": 10, "cache_read_input_tokens": 90, "output_tokens": 2 } } })));
        b.on_line(&ev(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } })));
        b.on_line(&ev(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "x".repeat(400) } })));
        assert_eq!(outputs(&rx), vec![2, 102], "streamed text is estimated at 4 chars a token");
        b.on_line(&ev(json!({ "type": "message_delta", "usage": { "output_tokens": 120 } })));
        b.on_line(&ev(json!({ "type": "message_stop" })));
        b.on_line(&ev(json!({ "type": "message_start", "message": { "id": "m2", "usage": { "input_tokens": 300, "output_tokens": 1 } } })));
        assert_eq!(outputs(&rx), vec![120, 121], "finished requests add up");
        b.on_line(&json!({ "type": "result", "total_cost_usd": 0.1, "usage": { "output_tokens": 130 }, "modelUsage": { "x": { "contextWindow": 1000 } } }));
        b.on_line(&ev(json!({ "type": "message_start", "message": { "id": "m3", "usage": { "output_tokens": 3 } } })));
        let us: Vec<Usage> = rx.try_iter().filter_map(|e| if let Ev::Usage(u) = e { Some(u) } else { None }).collect();
        assert_eq!(us.iter().map(|u| u.turn).collect::<Vec<_>>(), vec![130, 3], "the next turn counts from zero");
        assert_eq!(us.iter().map(|u| u.output).collect::<Vec<_>>(), vec![130, 133], "the session's output adds up");
        assert!(us[1].window == 1000 && us[1].cost == Some(0.1));
    }

    #[test]
    fn undo_rewinds_files_then_the_conversation() {
        let (mut b, rx) = bridge();
        let result = r#"{"type":"result","is_error":false,"usage":{}}"#;
        b.checkpoints.push(Checkpoint { uuid: "u1".into(), edited: false });
        b.on_line(&serde_json::from_str(r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"Write","input":{"file_path":"/x/a","content":""}}]}}"#).unwrap());
        b.on_line(&serde_json::from_str(result).unwrap());
        b.checkpoints.push(Checkpoint { uuid: "u2".into(), edited: false });
        b.on_line(&serde_json::from_str(result).unwrap());
        // Every user turn stays undoable; only edits mark it.
        assert_eq!(b.checkpoints.iter().map(|c| (c.uuid.as_str(), c.edited)).collect::<Vec<_>>(), [("u1", true), ("u2", false)]);
        // The last turn edited nothing: straight to the conversation.
        b.undoing = Some(Vec::new());
        b.on_line(&json!({"type":"control_response","response":{"subtype":"success","request_id":"undoconv1","response":{"rewound":true}}}));
        assert!(rx.try_iter().any(|e| matches!(e, Ev::Undone(Ok(f)) if f.is_empty())));
        assert_eq!(b.checkpoints.len(), 1);
        // The one before edited: its files are rewound, then the conversation.
        b.undoing = Some(Vec::new());
        b.on_line(&json!({"type":"control_response","response":{"subtype":"success","request_id":"undofiles2","response":{"canRewind":true,"filesChanged":["/x/a"]}}}));
        let out = b.outbox.pop().unwrap();
        assert_eq!((out["request"]["subtype"].as_str(), out["request"]["target_message_uuid"].as_str()), (Some("rewind_conversation"), Some("u1")));
        b.on_line(&json!({"type":"control_response","response":{"subtype":"success","request_id":"undoconv3","response":{"rewound":false,"error":"turn running","reason":"turn_running"}}}));
        assert!(rx.try_iter().any(|e| matches!(e, Ev::Undone(Err(m)) if m.contains("turn running") && m.contains("/x/a"))));
        assert_eq!(b.checkpoints.len(), 1);
        let id = new_uuid();
        assert_eq!((id.len(), &id[14..15]), (36, "4"));
    }

    #[test]
    fn background_bash_receipt_and_notification_emit_task_updates() {
        let (mut b, rx) = bridge();
        b.on_line(&json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"gh run watch 123","run_in_background":true}}]}}));
        b.on_line(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Command running in background with ID: b123. Output is being written to: /tmp/a.b/tasks/b123.output. You will be notified when it completes."}]}}));
        b.on_line(&json!({"type":"user","message":{"content":"<task-notification>\n<task-id>b123</task-id>\n<tool-use-id>t1</tool-use-id>\n<output-file>/tmp/a.b/tasks/b123.output</output-file>\n<status>completed</status>\n<summary>Background command completed (exit code 0)</summary>\n</task-notification>"}}));
        let events: Vec<_> = rx.try_iter().collect();
        let tasks: Vec<_> = events.iter().filter_map(|e| if let Ev::Task(t) = e { Some(t) } else { None }).collect();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "b123");
        assert_eq!(tasks[0].tool_id.as_deref(), Some("t1"));
        assert_eq!(tasks[0].output_file.as_deref(), Some("/tmp/a.b/tasks/b123.output"));
        assert_eq!(tasks[0].status.as_deref(), Some("running"));
        assert_eq!(tasks[1].status.as_deref(), Some("completed"));
        assert_eq!(tasks[1].exit_code, Some(0));
        assert!(shell_receipt("t", &json!({}), "ordinary output").is_none());
        assert!(shell_notification("ordinary message").is_none());
    }

    #[test]
    fn task_events_become_updates() {
        let started = json!({ "type": "system", "subtype": "task_started", "task_id": "a1", "tool_use_id": "toolu_1", "description": "Find auth code", "subagent_type": "Explore", "is_backgrounded": true });
        let t = task_event(&started).unwrap();
        assert_eq!((t.id.as_str(), t.tool_id.as_deref(), t.title.as_deref(), t.agent.as_deref(), t.status.as_deref()), ("a1", Some("toolu_1"), Some("Find auth code"), Some("Explore"), Some("running")));
        let progress = json!({ "type": "system", "subtype": "task_progress", "task_id": "a1", "usage": { "tool_uses": 3, "total_tokens": 900, "duration_ms": 5 }, "last_tool_name": "Grep" });
        let t = task_event(&progress).unwrap();
        assert_eq!((t.tools, t.last.as_deref(), t.status), (Some(3), Some("Grep"), None));
        let done = json!({ "type": "system", "subtype": "task_notification", "task_id": "a1", "status": "completed", "summary": "ok" });
        assert_eq!(task_event(&done).unwrap().status.as_deref(), Some("completed"));
        let updated = json!({ "type": "system", "subtype": "task_updated", "task_id": "a1", "patch": { "status": "killed" } });
        assert_eq!(task_event(&updated).unwrap().status.as_deref(), Some("killed"));
        assert!(task_event(&json!({ "type": "system", "subtype": "task_started" })).is_none());
    }

    #[test]
    fn task_tools_become_the_plan() {
        let mut p = Plan::default();
        assert!(p.created(&json!({ "subject": "Read the file", "description": "x" }), "Task #1 created successfully: Read the file"));
        assert!(p.created(&json!({ "subject": "Write the summary" }), "Task #2 created successfully: Write the summary"));
        assert!(p.update(&json!({ "taskId": "1", "status": "in_progress" })));
        assert!(p.update(&json!({ "taskId": 2, "subject": "Write a short summary" })));
        let t = p.todos();
        assert_eq!((t[0].content.as_str(), t[0].status.as_str()), ("Read the file", "in_progress"));
        assert_eq!((t[1].content.as_str(), t[1].status.as_str()), ("Write a short summary", "pending"));
        assert!(p.update(&json!({ "taskId": "1", "status": "deleted" })));
        assert_eq!(p.todos().len(), 1);
        assert!(!p.update(&json!({ "taskId": "9", "status": "completed" })));
        assert!(!p.created(&json!({ "subject": "x" }), "Error: no"));
        // Once everything is done, the next task starts a new plan.
        assert!(p.update(&json!({ "taskId": "2", "status": "completed" })));
        assert!(p.created(&json!({ "subject": "Next thing" }), "Task #3 created successfully: Next thing"));
        assert_eq!(p.todos().iter().map(|t| t.content.as_str()).collect::<Vec<_>>(), ["Next thing"]);
        assert_eq!(title("TaskUpdate", &json!({ "taskId": "1", "status": "in_progress" })), "#1 in progress");
    }
}
