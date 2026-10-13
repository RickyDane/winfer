//! winfer as an MCP server for the agents it runs, so one agent can orchestrate others:
//! list/read/message panes, start agents, open previews, ask the user for attention.
//!
//! Agents launch `winfer mcp-stdio` (this same binary) as a stdio MCP server. It forwards each
//! tool call to the running app over the token-protected hook server (`POST /mcp`); the app
//! runs it in the calling pane's workspace and replies. Agents still ask the user before using
//! MCP tools unless the user allows them.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

use crate::hooks;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    #[serde(default)]
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    /// Pane of the agent making the call; scopes it to that pane's workspace.
    pub pane_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    pub text: String,
}

fn tools() -> Value {
    let pane = json!({ "type": ["integer", "string"], "description": "Pane number as shown in winfer (1, 2, …) or pane id" });
    json!([
        {
            "name": "list_panes",
            "description": "List the terminal panes in your winfer workspace: number, agent, status (working, needs-input, done, idle, error), git branch and whether it is you.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "read_pane",
            "description": "Read the most recent output of another pane (its terminal screen and scrollback), e.g. to check what another agent did.",
            "inputSchema": { "type": "object", "properties": { "pane": pane, "lines": { "type": "integer", "description": "How many lines from the end (default 80, max 400)" } }, "required": ["pane"] }
        },
        {
            "name": "send_to_pane",
            "description": "Type a message into another pane, e.g. give another agent an instruction. Submits it unless submit is false.",
            "inputSchema": { "type": "object", "properties": { "pane": pane, "text": { "type": "string" }, "submit": { "type": "boolean" } }, "required": ["pane", "text"] }
        },
        {
            "name": "spawn_agent",
            "description": "Start a new agent pane in this workspace, optionally with a first prompt. By default it gets its own git worktree so it can't overwrite other agents' files. Providers: claude, codex, agy, grok, opencode, cursor, shell (only installed ones work).",
            "inputSchema": { "type": "object", "properties": {
                "provider": { "type": "string" },
                "prompt": { "type": "string" },
                "worktree": { "type": "boolean" }
            }, "required": ["provider"] }
        },
        {
            "name": "open_url",
            "description": "Open a URL in the user's winfer browser panel for this workspace, e.g. to show a dev server preview.",
            "inputSchema": { "type": "object", "properties": { "url": { "type": "string" }, "new_tab": { "type": "boolean" } }, "required": ["url"] }
        },
        {
            "name": "notify_user",
            "description": "Get the user's attention with a short message (desktop notification and pane highlight), e.g. when you need a decision.",
            "inputSchema": { "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }
        }
    ])
}

// ---------------------------------------------------------------------------------------------
// App side: calls arrive from the hook server, run in the webview, replies come back here.

static PENDING: OnceLock<Mutex<HashMap<String, mpsc::Sender<Reply>>>> = OnceLock::new();

fn pending() -> &'static Mutex<HashMap<String, mpsc::Sender<Reply>>> {
    PENDING.get_or_init(Mutex::default)
}

/// Forwards a call to the webview and waits for its reply.
pub fn dispatch(app: &AppHandle, mut call: Call) -> Reply {
    call.id = format!("{:016x}", rand::random::<u64>());
    let (tx, rx) = mpsc::channel();
    pending().lock().unwrap().insert(call.id.clone(), tx);
    if app.emit("mcp://call", &call).is_err() {
        pending().lock().unwrap().remove(&call.id);
        return Reply {
            ok: false,
            text: "winfer isn't ready".into(),
        };
    }
    let reply = rx.recv_timeout(Duration::from_secs(60)).unwrap_or(Reply {
        ok: false,
        text: "winfer didn't answer in time".into(),
    });
    pending().lock().unwrap().remove(&call.id);
    reply
}

#[tauri::command]
pub fn mcp_respond(id: String, reply: Reply) {
    if let Some(tx) = pending().lock().unwrap().remove(&id) {
        let _ = tx.send(reply);
    }
}

// ---------------------------------------------------------------------------------------------
// Agent side: `winfer mcp-stdio`.

fn forward(hook: &str, token: &str, call: &Call) -> Reply {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(90)))
        .http_status_as_error(false)
        .build()
        .into();
    let result = agent
        .post(&format!("http://{hook}/mcp"))
        .header("X-Winfer-Token", token)
        .send_json(call)
        .map_err(|e| e.to_string())
        .and_then(|mut r| r.body_mut().read_json::<Reply>().map_err(|e| e.to_string()));
    result.unwrap_or_else(|err| Reply {
        ok: false,
        text: format!("couldn't reach winfer: {err}"),
    })
}

fn handle(message: &Value, env: &(String, String, String)) -> Option<Value> {
    let id = message.get("id").cloned()?; // notifications get no response
    let method = message["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "winfer", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "You are running inside winfer, a workspace of terminal panes with other coding agents. Use these tools to coordinate with them and to show things to the user."
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let (hook, token, pane) = env;
            let call = Call {
                id: String::new(),
                tool: message["params"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
                args: message["params"]["arguments"].clone(),
                pane_id: pane.clone(),
            };
            let reply = forward(hook, token, &call);
            json!({ "content": [{ "type": "text", "text": reply.text }], "isError": !reply.ok })
        }
        _ => {
            return Some(
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {method}") } }),
            );
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Entry point for `winfer mcp-stdio`: newline-delimited JSON-RPC on stdin/stdout.
pub fn run_stdio() {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let env = (
        var("WINFER_HOOK"),
        var("WINFER_TOKEN"),
        var("WINFER_PANE_ID"),
    );
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(response) = handle(&message, &env) {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{response}");
            let _ = out.flush();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Wiring MCP servers into agent CLIs.

pub struct Server {
    pub name: &'static str,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(&'static str, String)>,
}

/// The winfer server for a pane, if the hook server is running.
pub fn winfer_server(pane_id: &str) -> Option<Server> {
    let hooks = hooks::server()?;
    let exe = std::env::current_exe().ok()?;
    Some(Server {
        name: "winfer",
        command: exe.to_string_lossy().into_owned(),
        args: vec!["mcp-stdio".into()],
        env: vec![
            ("WINFER_HOOK", hooks.addr.clone()),
            ("WINFER_TOKEN", hooks.token.clone()),
            ("WINFER_PANE_ID", pane_id.to_string()),
        ],
    })
}

/// CLI arguments that register `servers` with an agent (Claude Code or Codex).
pub fn agent_args(provider_id: &str, servers: &[Server]) -> Vec<String> {
    if servers.is_empty() {
        return Vec::new();
    }
    match provider_id {
        "claude" => {
            let config: serde_json::Map<String, Value> = servers
                .iter()
                .map(|s| {
                    let env: serde_json::Map<String, Value> = s
                        .env
                        .iter()
                        .map(|(k, v)| (k.to_string(), json!(v)))
                        .collect();
                    (
                        s.name.to_string(),
                        json!({ "command": s.command, "args": s.args, "env": env }),
                    )
                })
                .collect();
            vec![
                "--mcp-config".into(),
                json!({ "mcpServers": config }).to_string(),
            ]
        }
        "codex" => servers
            .iter()
            .flat_map(|s| {
                let key = s.name.replace('-', "_");
                let env = s
                    .env
                    .iter()
                    .map(|(k, v)| format!("{k}={}", json!(v)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut args = vec![
                    "-c".to_string(),
                    format!("mcp_servers.{key}.command={}", json!(s.command)),
                    "-c".into(),
                    format!("mcp_servers.{key}.args={}", json!(s.args)),
                ];
                if !env.is_empty() {
                    args.extend(["-c".into(), format!("mcp_servers.{key}.env={{{env}}}")]);
                }
                args
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> (String, String, String) {
        ("127.0.0.1:1".into(), "t".into(), "p-1".into())
    }

    #[test]
    fn speaks_mcp_lifecycle() {
        let init = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }), &env()).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert!(
            handle(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                &env()
            )
            .is_none()
        );

        let list = handle(
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            &env(),
        )
        .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_panes",
                "read_pane",
                "send_to_pane",
                "spawn_agent",
                "open_url",
                "notify_user"
            ]
        );

        let unknown = handle(
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }),
            &env(),
        )
        .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn tool_errors_are_reported_not_thrown() {
        // Nothing listens on port 1: the call must come back as an MCP tool error.
        let res = handle(&json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "list_panes", "arguments": {} } }), &env()).unwrap();
        assert_eq!(res["result"]["isError"], true);
        assert!(
            res["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("couldn't reach winfer")
        );
    }

    #[test]
    fn registers_servers_per_cli() {
        let servers = [Server {
            name: "winfer",
            command: "/app/winfer".into(),
            args: vec!["mcp-stdio".into()],
            env: vec![("WINFER_PANE_ID", "p-1".into())],
        }];
        let claude = agent_args("claude", &servers);
        let config: Value = serde_json::from_str(&claude[1]).unwrap();
        assert_eq!(
            config["mcpServers"]["winfer"]["env"]["WINFER_PANE_ID"],
            "p-1"
        );
        let codex = agent_args("codex", &servers);
        assert!(codex.contains(&r#"mcp_servers.winfer.command="/app/winfer""#.to_string()));
        assert!(codex.contains(&r#"mcp_servers.winfer.env={WINFER_PANE_ID="p-1"}"#.to_string()));
        assert!(agent_args("agy", &servers).is_empty());
    }
}
