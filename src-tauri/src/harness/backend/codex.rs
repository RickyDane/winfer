//! Codex bridge: `codex app-server` (JSON-RPC over stdio), with Codex's own tools, sandbox and
//! login. Approval requests from the server are answered by the TUI.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::{Value, json};

use super::{In, LineProcess, Opts};
use crate::harness::event::{Approval, Cmd, Decision, Ev, Events, Mode, ModelInfo, Todo, Usage, UserMsg};

fn approval_policy(mode: Mode) -> &'static str {
    match mode {
        Mode::Yolo => "never",
        _ => "on-request",
    }
}

fn sandbox(mode: Mode) -> &'static str {
    match mode {
        Mode::Yolo => "danger-full-access",
        Mode::Plan => "read-only",
        _ => "workspace-write",
    }
}

fn sandbox_policy(mode: Mode, cwd: &str) -> Value {
    match mode {
        Mode::Yolo => json!({ "type": "dangerFullAccess" }),
        Mode::Plan => json!({ "type": "readOnly", "networkAccess": false }),
        _ => json!({ "type": "workspaceWrite", "writableRoots": [cwd], "networkAccess": false, "excludeTmpdirEnvVar": false, "excludeSlashTmp": false }),
    }
}

/// `-c` flags that register MCP servers (`{ name: { command, args, env } }`) with Codex.
fn mcp_args(servers: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut args = Vec::new();
    for (name, s) in servers {
        let key = name.replace('-', "_");
        args.extend(["-c".to_string(), format!("mcp_servers.{key}.command={}", s["command"])]);
        args.extend(["-c".to_string(), format!("mcp_servers.{key}.args={}", s["args"])]);
        if let Some(env) = s["env"].as_object().filter(|e| !e.is_empty()) {
            let pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            args.extend(["-c".to_string(), format!("mcp_servers.{key}.env={{{}}}", pairs.join(", "))]);
        }
    }
    args
}

enum Pending {
    Init,
    Thread,
    Turn,
    Ignore,
}

struct Bridge {
    events: Events,
    proc: LineProcess,
    next: u64,
    pending: HashMap<u64, Pending>,
    thread: Option<String>,
    turn: Option<String>,
    queued: Vec<UserMsg>,
    mode: Mode,
    model: Option<String>,
    effort: Option<String>,
    cwd: String,
    /// Approval requests waiting on the user: our id → (JSON-RPC id, kind).
    approvals: HashMap<String, (Value, &'static str)>,
    /// Diffs of file-change items, for their result card.
    diffs: HashMap<String, String>,
}

pub fn run(opts: Opts, rx: Receiver<In>, me: Sender<In>, events: Events) {
    let mut args = vec!["app-server".to_string()];
    args.extend(mcp_args(&opts.mcp));
    let proc = match LineProcess::spawn("codex", &args, &[], &opts.cwd, me) {
        Ok(p) => p,
        Err(e) => {
            let _ = events.send(Ev::Exited(e));
            return;
        }
    };
    let cwd = opts.cwd.to_string_lossy().into_owned();
    let mut b = Bridge {
        events,
        proc,
        next: 0,
        pending: HashMap::new(),
        thread: None,
        turn: None,
        queued: Vec::new(),
        mode: opts.mode,
        model: Some(opts.model.clone()).filter(|m| !m.is_empty()),
        effort: opts.effort.clone(),
        cwd: cwd.clone(),
        approvals: HashMap::new(),
        diffs: HashMap::new(),
    };
    b.request("initialize", json!({ "clientInfo": { "name": "winfer", "title": "winfer code", "version": env!("CARGO_PKG_VERSION") }, "capabilities": null }), Pending::Init);
    let mut thread_params = json!({
        "cwd": cwd,
        "approvalPolicy": approval_policy(opts.mode),
        "sandbox": sandbox(opts.mode),
    });
    if let Some(m) = &b.model {
        thread_params["model"] = json!(m);
    }
    if let Some(system) = &opts.system {
        thread_params["developerInstructions"] = json!(system);
    }
    let resume = opts.resume.clone();

    while let Some(msg) = super::recv(&rx) {
        match msg {
            In::Line(line) => {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(id) = v.get("id").filter(|_| v.get("method").is_none()) {
                    // A response to one of our requests.
                    let kind = id.as_u64().and_then(|id| b.pending.remove(&id));
                    if let Some(err) = v.get("error") {
                        let message = err["message"].as_str().unwrap_or("request failed").to_string();
                        if matches!(kind, Some(Pending::Turn)) {
                            b.emit(Ev::Error(format!("Codex: {message}")));
                            b.emit(Ev::TurnDone);
                        } else if matches!(kind, Some(Pending::Thread)) && resume.is_some() {
                            b.emit(Ev::Notice(format!("Couldn't resume that Codex thread ({message}); starting a new one.")));
                            b.request("thread/start", thread_params.clone(), Pending::Thread);
                        } else {
                            b.emit(Ev::Error(format!("Codex: {message}")));
                        }
                        continue;
                    }
                    let result = &v["result"];
                    match kind {
                        Some(Pending::Init) => {
                            let _ = b.proc.write(&json!({ "method": "initialized" }));
                            match &resume {
                                Some(thread) => {
                                    let mut p = thread_params.clone();
                                    p["threadId"] = json!(thread);
                                    p["excludeTurns"] = json!(true);
                                    b.request("thread/resume", p, Pending::Thread);
                                }
                                None => b.request("thread/start", thread_params.clone(), Pending::Thread),
                            }
                        }
                        Some(Pending::Thread) => {
                            let id = result["thread"]["id"].as_str().unwrap_or("").to_string();
                            b.emit(Ev::Session(id.clone()));
                            if let Some(m) = result["model"].as_str() {
                                b.emit(Ev::Model(m.to_string()));
                            }
                            b.thread = Some(id);
                            for msg in std::mem::take(&mut b.queued) {
                                b.start_turn(msg);
                            }
                        }
                        Some(Pending::Turn) => {
                            if let Some(id) = result["turn"]["id"].as_str() {
                                b.turn = Some(id.to_string());
                            }
                        }
                        _ => {}
                    }
                } else if let Some(method) = v["method"].as_str() {
                    match v.get("id") {
                        Some(id) => b.on_request(method, id.clone(), &v["params"]),
                        None => b.on_notification(method, &v["params"]),
                    }
                }
            }
            In::Closed => {
                let reason = b.proc.exit_reason();
                b.emit(Ev::Exited(format!("Codex stopped: {reason}")));
                return;
            }
            In::Cmd(cmd) => match cmd {
                Cmd::Send(msg) => {
                    b.emit(Ev::TurnStart);
                    if b.thread.is_some() {
                        b.start_turn(msg);
                    } else {
                        b.queued.push(msg);
                    }
                }
                Cmd::Interrupt => {
                    if let (Some(thread), Some(turn)) = (b.thread.clone(), b.turn.clone()) {
                        b.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn }), Pending::Ignore);
                    }
                }
                Cmd::Approve(id, decision) => {
                    let Some((rpc, _kind)) = b.approvals.remove(&id) else { continue };
                    let decision = match decision {
                        Decision::Allow => "accept",
                        Decision::Always => "acceptForSession",
                        Decision::Deny => "decline",
                    };
                    let _ = b.proc.write(&json!({ "id": rpc, "result": { "decision": decision } }));
                }
                Cmd::SetModel(m) => b.model = Some(m),
                Cmd::SetEffort(e) => b.effort = e,
                Cmd::SetMode(m) => b.mode = m,
                Cmd::Compact => {
                    if let Some(thread) = b.thread.clone() {
                        b.emit(Ev::TurnStart);
                        b.request("thread/compact/start", json!({ "threadId": thread }), Pending::Ignore);
                    }
                }
                Cmd::SetCompaction { .. } => {}
                Cmd::Undo => b.emit(Ev::Notice("Undo isn't available with Codex; ask it to revert instead.".into())),
                Cmd::Shutdown => return,
            },
        }
    }
}

impl Bridge {
    fn emit(&self, ev: Ev) {
        let _ = self.events.send(ev);
    }

    fn request(&mut self, method: &str, params: Value, kind: Pending) {
        self.next += 1;
        self.pending.insert(self.next, kind);
        let _ = self.proc.write(&json!({ "id": self.next, "method": method, "params": params }));
    }

    fn start_turn(&mut self, msg: UserMsg) {
        let Some(thread) = self.thread.clone() else { return };
        let mut input = vec![json!({ "type": "text", "text": msg.text, "text_elements": [] })];
        for path in &msg.images {
            input.push(json!({ "type": "localImage", "path": path }));
        }
        let mut params = json!({
            "threadId": thread,
            "input": input,
            "approvalPolicy": approval_policy(self.mode),
            "sandboxPolicy": sandbox_policy(self.mode, &self.cwd),
        });
        if let Some(m) = &self.model {
            params["model"] = json!(m);
        }
        if let Some(e) = &self.effort {
            params["effort"] = json!(e);
        }
        self.request("turn/start", params, Pending::Turn);
    }

    fn on_request(&mut self, method: &str, id: Value, p: &Value) {
        let key = format!("codex-{id}");
        let approval = match method {
            "item/commandExecution/requestApproval" | "execCommandApproval" => {
                let command = p["command"].as_str().map(strip_shell).unwrap_or_default();
                Some(Approval {
                    id: key.clone(),
                    tool: "Bash".into(),
                    title: command,
                    detail: p["reason"].as_str().unwrap_or("").to_string(),
                    diff: None,
                })
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => {
                let item = p["itemId"].as_str().unwrap_or("");
                Some(Approval {
                    id: key.clone(),
                    tool: "Edit".into(),
                    title: p["grantRoot"].as_str().map(|r| format!("write access to {r}")).unwrap_or_else(|| "apply changes".into()),
                    detail: p["reason"].as_str().unwrap_or("").to_string(),
                    diff: self.diffs.get(item).cloned(),
                })
            }
            "item/permissions/requestApproval" => Some(Approval {
                id: key.clone(),
                tool: "Permissions".into(),
                title: p["reason"].as_str().unwrap_or("more access").to_string(),
                detail: String::new(),
                diff: None,
            }),
            _ => None,
        };
        match approval {
            Some(a) => {
                self.approvals.insert(key, (id, "approval"));
                self.emit(Ev::Approval(a));
            }
            None => {
                // Requests we don't support (user input forms, auth refresh): refuse politely.
                let _ = self.proc.write(&json!({ "id": id, "error": { "code": -32601, "message": format!("winfer code doesn't support {method}") } }));
            }
        }
    }

    fn on_notification(&mut self, method: &str, p: &Value) {
        match method {
            "turn/started" => self.turn = p["turn"]["id"].as_str().map(str::to_string),
            "turn/completed" => {
                let turn = &p["turn"];
                if turn["status"] == "failed" {
                    let message = turn["error"]["message"].as_str().unwrap_or("the turn failed");
                    self.emit(Ev::Error(format!("Codex: {message}")));
                }
                self.turn = None;
                self.emit(Ev::TurnDone);
            }
            "error" => {
                if p["willRetry"].as_bool() != Some(true) {
                    let message = p["error"]["message"].as_str().unwrap_or("error");
                    self.emit(Ev::Error(format!("Codex: {message}")));
                }
            }
            "item/agentMessage/delta" => self.emit(Ev::TextDelta { id: s(p, "itemId"), text: s(p, "delta") }),
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => self.emit(Ev::ReasoningDelta { id: s(p, "itemId"), text: s(p, "delta") }),
            "item/reasoning/summaryPartAdded" => self.emit(Ev::ReasoningDelta { id: s(p, "itemId"), text: "\n\n".into() }),
            "item/commandExecution/outputDelta" => self.emit(Ev::ToolOutput { id: s(p, "itemId"), chunk: s(p, "delta") }),
            "turn/plan/updated" => {
                let todos = p["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|step| Todo {
                        content: step["step"].as_str().unwrap_or("").to_string(),
                        status: match step["status"].as_str() {
                            Some("inProgress") => "in_progress".into(),
                            Some(other) => other.to_string(),
                            None => "pending".into(),
                        },
                    })
                    .collect();
                self.emit(Ev::Todos(todos));
            }
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"];
                let n = |part: &str, k: &str| u[part][k].as_u64().unwrap_or(0);
                self.emit(Ev::Usage(Usage {
                    context: n("last", "inputTokens") + n("last", "outputTokens"),
                    window: u["modelContextWindow"].as_u64().unwrap_or(272_000),
                    input: n("total", "inputTokens"),
                    output: n("total", "outputTokens"),
                    cached: n("total", "cachedInputTokens"),
                    cost: None,
                }));
            }
            "thread/compacted" => self.emit(Ev::Notice("Context compacted.".into())),
            "model/rerouted" => {
                if let Some(m) = p["toModel"].as_str().or_else(|| p["model"].as_str()) {
                    self.emit(Ev::Model(m.to_string()));
                }
            }
            "item/started" => self.item_started(&p["item"]),
            "item/completed" => self.item_completed(&p["item"]),
            _ => {}
        }
    }

    fn item_started(&mut self, item: &Value) {
        let id = s(item, "id");
        let (name, title) = match item["type"].as_str().unwrap_or("") {
            "commandExecution" => ("Bash".to_string(), strip_shell(item["command"].as_str().unwrap_or(""))),
            "fileChange" => {
                let diff = changes_diff(item);
                self.diffs.insert(id.clone(), diff);
                ("Edit".into(), changed_paths(item))
            }
            "mcpToolCall" => (format!("{} · {}", s(item, "server"), s(item, "tool")), first_arg(&item["arguments"])),
            "dynamicToolCall" => (s(item, "tool"), first_arg(&item["arguments"])),
            "webSearch" => ("WebSearch".into(), s(item, "query")),
            "collabAgentToolCall" => ("Agent".into(), s(item, "prompt").lines().next().unwrap_or("").to_string()),
            "imageView" => ("ViewImage".into(), crate::harness::tools::short_path(&s(item, "path"))),
            _ => return,
        };
        self.emit(Ev::ToolStart { id, name, title, input: item.clone() });
    }

    fn item_completed(&mut self, item: &Value) {
        let id = s(item, "id");
        match item["type"].as_str().unwrap_or("") {
            "agentMessage" => self.emit(Ev::Text { id, text: s(item, "text") }),
            "commandExecution" => {
                let ok = item["exitCode"].as_i64() == Some(0) || item["status"] == "completed" && item["exitCode"].is_null();
                self.emit(Ev::ToolEnd { id, ok, output: s(item, "aggregatedOutput"), diff: None });
            }
            "fileChange" => {
                let diff = changes_diff(item);
                self.diffs.remove(&id);
                let ok = item["status"] == "completed";
                self.emit(Ev::ToolEnd { id, ok, output: String::new(), diff: Some(diff).filter(|d| !d.is_empty()) });
            }
            "mcpToolCall" | "dynamicToolCall" => {
                let ok = item["error"].is_null();
                let output = if ok {
                    item["result"]["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    item["error"]["message"].as_str().unwrap_or("failed").to_string()
                };
                self.emit(Ev::ToolEnd { id, ok, output, diff: None });
            }
            "webSearch" | "collabAgentToolCall" | "imageView" => self.emit(Ev::ToolEnd { id, ok: true, output: String::new(), diff: None }),
            "contextCompaction" => self.emit(Ev::Notice("Context compacted.".into())),
            _ => {}
        }
    }
}

// No lean mode for Codex: its default instructions and tool list are a prefix OpenAI's prompt
// cache already holds, so a shorter custom setup measured *more* full-price tokens (bench: 8.6k
// stock vs 26.1k with a lean prompt, 20.4k with just unused features off).

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn first_arg(args: &Value) -> String {
    args.as_object()
        .and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default()
}

/// `/bin/zsh -lc 'ls -la'` → `ls -la`.
fn strip_shell(command: &str) -> String {
    for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "bash -lc ", "zsh -lc ", "/bin/sh -c "] {
        if let Some(rest) = command.strip_prefix(prefix) {
            let rest = rest.trim();
            let unquoted = rest
                .strip_prefix('\'')
                .and_then(|r| r.strip_suffix('\''))
                .or_else(|| rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')));
            return unquoted.unwrap_or(rest).to_string();
        }
    }
    command.to_string()
}

fn changed_paths(item: &Value) -> String {
    item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["path"].as_str())
        .map(crate::harness::tools::short_path)
        .collect::<Vec<_>>()
        .join(", ")
}

fn changes_diff(item: &Value) -> String {
    let mut out = String::new();
    for c in item["changes"].as_array().into_iter().flatten() {
        let path = crate::harness::tools::short_path(c["path"].as_str().unwrap_or(""));
        let raw = c["diff"].as_str().unwrap_or("");
        // New and deleted files carry their content, not a diff.
        let whole = match c["kind"]["type"].as_str() {
            Some("add") => Some(crate::harness::diff::unified(&path, "", raw)),
            Some("delete") => Some(crate::harness::diff::unified(&path, raw, "")),
            _ => None,
        };
        if let Some(d) = whole {
            out.push_str(&d);
            continue;
        }
        let diff = raw;
        if !diff.starts_with("---") {
            out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
        }
        out.push_str(diff);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Models from `model/list`, through a short-lived app server.
pub fn models() -> Vec<ModelInfo> {
    if crate::env::find_command("codex").is_none() {
        return Vec::new();
    }
    let (tx, rx) = channel();
    let cwd = std::env::temp_dir();
    let Ok(mut proc) = LineProcess::spawn("codex", &["app-server".to_string()], &[], &cwd, tx) else {
        return Vec::new();
    };
    let _ = proc.write(&json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "winfer", "title": null, "version": env!("CARGO_PKG_VERSION") }, "capabilities": null } }));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(In::Line(line)) = rx.recv_timeout(left) else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["id"].as_u64() {
            Some(1) => {
                let _ = proc.write(&json!({ "method": "initialized" }));
                let _ = proc.write(&json!({ "id": 2, "method": "model/list", "params": {} }));
            }
            Some(2) => {
                return v["result"]["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|m| m["hidden"].as_bool() != Some(true))
                    .map(|m| ModelInfo {
                        spec: format!("codex/{}", s(m, "model")),
                        label: m["displayName"].as_str().unwrap_or(m["model"].as_str().unwrap_or("")).to_string(),
                        detail: format!("Codex · {}", s(m, "description")),
                    })
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_shell_wrappers() {
        assert_eq!(strip_shell("/bin/zsh -lc 'curl -s x | head'"), "curl -s x | head");
        assert_eq!(strip_shell("/bin/zsh -lc \"printf 'hi' > a.txt\""), "printf 'hi' > a.txt");
        assert_eq!(strip_shell("ls"), "ls");
    }

    #[test]
    fn mcp_servers_become_config_flags() {
        let servers: serde_json::Map<String, Value> =
            serde_json::from_value(json!({ "winfer-x": { "command": "/w", "args": ["mcp-stdio"], "env": { "A": "1" } } })).unwrap();
        let args = mcp_args(&servers);
        assert_eq!(args[1], r#"mcp_servers.winfer_x.command="/w""#);
        assert_eq!(args[3], r#"mcp_servers.winfer_x.args=["mcp-stdio"]"#);
        assert_eq!(args[5], r#"mcp_servers.winfer_x.env={A="1"}"#);
    }
}
