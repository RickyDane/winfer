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
            "description": "Start a new agent pane in this workspace, optionally with a first prompt. By default it gets its own git worktree so it can't overwrite other agents' files. Providers: claude, codex, agy, grok, opencode, cursor, pi, shell (only installed ones work).",
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
            "name": "open_file",
            "description": "Show a file in the user's winfer file viewer (a side panel tab next to the browser): markdown is rendered, images shown, other text files with line numbers. Use it to present a plan, report or doc you wrote. The view follows later edits to the file. The user can edit a markdown file there and leave comments in it as HTML comments like `<!-- @user: ... -->` after the part they are about: read the file again to see them, address them, and remove a comment once it's handled (or reply with `<!-- @agent: ... -->`).",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Absolute, or relative to your working directory" },
                "line": { "type": "number", "description": "Line to scroll to" }
            }, "required": ["path"] }
        },
        {
            "name": "notify_user",
            "description": "Get the user's attention with a short message (desktop notification and pane highlight), e.g. when you need a decision.",
            "inputSchema": { "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }
        },
        {
            "name": "usage_status",
            "description": "How much of your plan's usage limits is left right now: each window (5-hour session, weekly, …) with percent used and left, when it resets, and when it runs out at the current pace. Check it before and during long tasks, e.g. when told to pause if usage runs low.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "wait_for_usage_reset",
            "description": "Pause until your usage limits reset: winfer types a message into your pane when they do, so you can continue. Call it when usage_status shows you are running low, then end your turn and don't work, poll or sleep until the message arrives. It waits for the windows with less than `below` percent left (or, if none, the one with least left).",
            "inputSchema": { "type": "object", "properties": {
                "below": { "type": "number", "description": "Wait for the reset of windows with less than this percent left (default 20)" },
                "minutes": { "type": "number", "description": "Wait this many minutes instead of until a reset" },
                "message": { "type": "string", "description": "What winfer types to wake you, e.g. a reminder of what to continue with (default: usage has reset, continue where you left off)" },
                "cancel": { "type": "boolean", "description": "Cancel your pending wake-up instead" }
            } }
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
    // Long enough for the user to answer an approval dialog a plugin tool shows.
    let reply = rx.recv_timeout(Duration::from_secs(300)).unwrap_or(Reply {
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
        .timeout_global(Some(Duration::from_secs(330)))
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

/// Built-in tools plus those enabled plugins offer agents (asked from the app; built-ins only
/// when it can't be reached).
fn all_tools(env: &(String, String, String)) -> Value {
    let mut list = tools();
    let (hook, token, pane) = env;
    let call = Call { id: String::new(), tool: LIST_PLUGIN_TOOLS.into(), args: json!({}), pane_id: pane.clone() };
    let reply = forward(hook, token, &call);
    if let (true, Ok(Value::Array(extra))) = (reply.ok, serde_json::from_str::<Value>(&reply.text)) {
        let builtin: Vec<Value> = list.as_array().unwrap().iter().map(|t| t["name"].clone()).collect();
        let extra = extra.into_iter().filter(|t| t["name"].is_string() && !builtin.contains(&t["name"]));
        list.as_array_mut().unwrap().extend(extra);
    }
    list
}

/// Internal call the stdio server makes to list plugin tools (see agentApi.ts).
const LIST_PLUGIN_TOOLS: &str = "__plugin_tools";

fn handle(message: &Value, env: &(String, String, String)) -> Option<Value> {
    let id = message.get("id").cloned()?; // notifications get no response
    let method = message["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "winfer", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "You are running inside winfer, a workspace of terminal panes with other coding agents. Use these tools to coordinate with them and to show things to the user. usage_status tells you exactly how much of your plan's usage is left; wait_for_usage_reset pauses you until it resets."
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": all_tools(env) }),
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
    let hidden = hidden_tools();
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(mut response) = handle(&message, &env) {
            hide_tools(&mut response, &hidden);
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

/// The winfer server for a pane, if the hook server is running, without the `hidden` tools.
pub fn winfer_server(pane_id: &str, hidden: &[String]) -> Option<Server> {
    let hooks = hooks::server()?;
    let exe = std::env::current_exe().ok()?;
    let mut env = vec![
        ("WINFER_HOOK", hooks.addr.clone()),
        ("WINFER_TOKEN", hooks.token.clone()),
        ("WINFER_PANE_ID", pane_id.to_string()),
    ];
    if !hidden.is_empty() {
        env.push((HIDE_ENV, hidden.join(",")));
    }
    Some(Server { name: "winfer", command: exe.to_string_lossy().into_owned(), args: vec!["mcp-stdio".into()], env })
}

// ---------------------------------------------------------------------------------------------
// codegraph (https://github.com/colbymchenry/codegraph): in a project the user indexed with it,
// agents get its own MCP server the way codegraph intends, not winfer's kb_* wrappers around its
// CLI: one always-loaded `codegraph_explore` tool that returns the relevant symbols' source,
// call paths and blast radius in one call, with codegraph's own usage instructions and a file
// watcher that keeps the index fresh while the agent edits.

/// Knowledge Base tools `codegraph_explore` covers; left out of winfer's server when it's attached.
pub const CODEGRAPH_COVERS: [&str; 4] = ["kb_query", "kb_explain", "kb_path", "kb_affected"];
const CODEGRAPH: &str = "codegraph";

/// The project folder `dir` is in, if it has a codegraph index (`.codegraph/codegraph.db`).
/// Looks up to the repository root, so a pane opened in a subfolder still finds it.
fn codegraph_root(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for d in dir.ancestors() {
        if d.join(".codegraph").join("codegraph.db").is_file() {
            return Some(d.to_path_buf());
        }
        if d.join(".git").exists() {
            return None;
        }
    }
    None
}

/// codegraph's MCP server for the project `dir` is in, if it's indexed and codegraph is installed.
pub fn codegraph_server(dir: &std::path::Path) -> Option<Server> {
    let root = codegraph_root(dir)?;
    // A .cmd shim would hand its arguments to cmd.exe; the launcher resolves npm's to node.
    let launcher = crate::env::launcher(CODEGRAPH).filter(|l| !l.batch)?;
    let mut args = launcher.args.clone();
    args.extend(["serve", "--mcp", "--path"].map(String::from));
    args.push(root.to_string_lossy().into_owned());
    Some(Server {
        name: CODEGRAPH,
        command: launcher.program.to_string_lossy().into_owned(),
        args,
        env: vec![("CODEGRAPH_TELEMETRY", "0".into()), ("CODEGRAPH_NO_UPDATE_CHECK", "1".into())],
    })
}

// ---------------------------------------------------------------------------------------------
// Hiding tools: each one an agent is offered costs context in every request, so the ones the
// user turned off (Settings → Agent tools) are left out of `tools/list`. winfer's own server
// reads them from its environment; other servers (the browser's Playwright MCP, which can't
// leave tools out itself) run behind `winfer mcp-filter`, which drops them from the replies.

/// Tool names to hide, comma-separated, for `winfer mcp-stdio`.
const HIDE_ENV: &str = "WINFER_MCP_HIDE";

fn hidden_tools() -> Vec<String> {
    std::env::var(HIDE_ENV).unwrap_or_default().split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect()
}

/// Drops the `hidden` tools from a `tools/list` reply; other messages pass unchanged.
fn hide_tools(message: &mut Value, hidden: &[String]) -> bool {
    let Some(tools) = message.pointer_mut("/result/tools").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = tools.len();
    tools.retain(|t| !t["name"].as_str().is_some_and(|n| hidden.iter().any(|h| h == n)));
    tools.len() != before
}

/// `server` started through `winfer mcp-filter`, which hides the `hidden` tools of it.
pub fn filtered(server: Server, hidden: &[String]) -> Server {
    let Some(exe) = std::env::current_exe().ok().filter(|_| !hidden.is_empty()) else {
        return server;
    };
    let mut args = vec!["mcp-filter".to_string(), hidden.join(","), "--".into(), server.command];
    args.extend(server.args);
    Server { name: server.name, command: exe.to_string_lossy().into_owned(), args, env: server.env }
}

/// Entry point for `winfer mcp-filter <tools> -- <command> [args]`: runs the MCP server
/// `command` with stdin passed through and its `tools/list` replies without `tools`
/// (comma-separated). Exits with the server's exit code.
pub fn run_filter(args: &[String]) -> i32 {
    let (Some(hidden), Some("--"), Some(command)) = (args.first(), args.get(1).map(String::as_str), args.get(2)) else {
        eprintln!("usage: winfer mcp-filter <tools> -- <command> [args]");
        return 2;
    };
    let hidden: Vec<String> = hidden.split(',').filter(|t| !t.is_empty()).map(String::from).collect();
    let mut cmd = std::process::Command::new(command);
    cmd.args(&args[3..]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    // From the GUI binary on Windows, a console program (`cmd /c npx`) would open a window.
    let mut child = match crate::env::no_window(&mut cmd).spawn() {
        Ok(child) => child,
        Err(err) => {
            eprintln!("winfer mcp-filter: couldn't start {command}: {err}");
            return 1;
        }
    };
    let mut to_child = child.stdin.take().expect("piped stdin");
    // The agent's requests go through as they are; when it closes stdin, so does the server's.
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_child);
    });
    let from_child = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
    let stdout = std::io::stdout();
    for line in from_child.lines().map_while(Result::ok) {
        let mut out = stdout.lock();
        // Only a reply that lists tools is rewritten; everything else is passed on byte for byte.
        let rewritten = line.contains("\"tools\"").then(|| serde_json::from_str::<Value>(&line).ok()).flatten().and_then(|mut v| hide_tools(&mut v, &hidden).then(|| v.to_string()));
        let _ = writeln!(out, "{}", rewritten.as_deref().unwrap_or(&line));
        let _ = out.flush();
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(1)
}

/// CLI arguments that register `servers` with an agent (Claude Code or Codex; OpenCode and pi:
/// [`agent_env`]).
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
                    let mut entry = json!({ "command": s.command, "args": s.args, "env": env });
                    // Claude Code defers MCP tools behind a tool search; codegraph's one tool is
                    // meant to be at hand from the first prompt.
                    if s.name == CODEGRAPH {
                        entry["alwaysLoad"] = json!(true);
                    }
                    (s.name.to_string(), entry)
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

/// Environment that registers `servers` (and winfer's status `plugin`, a directory) with an
/// agent configured through its environment (OpenCode reads extra config from
/// `OPENCODE_CONFIG_CONTENT`). `existing` is the variable's current value, if the user set one:
/// ours are added to it. pi's winfer extension registers the servers in `WINFER_PI_MCP`.
pub fn agent_env(provider_id: &str, servers: &[Server], plugin: Option<&str>, auto_approve: bool, existing: Option<&str>) -> Vec<(String, String)> {
    if provider_id == "pi" || provider_id == "winfer" {
        if servers.is_empty() {
            return Vec::new();
        }
        let config: serde_json::Map<String, Value> = servers
            .iter()
            .map(|s| {
                let env: serde_json::Map<String, Value> = s.env.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
                let mut entry = json!({ "command": s.command, "args": s.args, "env": env });
                // winfer code's Claude backend hands these to `--mcp-config` as they are.
                if s.name == CODEGRAPH {
                    entry["alwaysLoad"] = json!(true);
                }
                (s.name.to_string(), entry)
            })
            .collect();
        let var = if provider_id == "pi" { "WINFER_PI_MCP" } else { "WINFER_HARNESS_MCP" };
        return vec![(var.into(), Value::Object(config).to_string())];
    }
    if (servers.is_empty() && plugin.is_none() && !auto_approve) || provider_id != "opencode" {
        return Vec::new();
    }
    let mut config = existing
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if !config["mcp"].is_object() {
        config["mcp"] = json!({});
    }
    for s in servers {
        let environment: serde_json::Map<String, Value> = s
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        let command: Vec<&str> = std::iter::once(s.command.as_str()).chain(s.args.iter().map(String::as_str)).collect();
        config["mcp"][s.name] = json!({
            "type": "local",
            "command": command,
            "environment": environment,
            "enabled": true,
        });
    }
    if let Some(plugin) = plugin {
        if !config["plugins"].is_array() {
            config["plugins"] = json!([]);
        }
        config["plugins"].as_array_mut().expect("an array").push(json!(plugin));
    }
    // Like OpenCode 2's `--auto`: allow what the user's config doesn't explicitly set.
    if auto_approve {
        if !config["permission"].is_object() && !config["permission"].is_string() {
            config["permission"] = json!({});
        }
        if let Some(permission) = config["permission"].as_object_mut() {
            for tool in ["edit", "bash", "webfetch"] {
                permission.entry(tool).or_insert(json!("allow"));
            }
        }
    }
    vec![("OPENCODE_CONFIG_CONTENT".into(), config.to_string())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_gets_servers_through_its_config_env() {
        let servers = [Server {
            name: "winfer",
            command: "/app/winfer".into(),
            args: vec!["mcp-stdio".into()],
            env: vec![("WINFER_PANE_ID", "p-1".into())],
        }];
        let env = agent_env("opencode", &servers, Some("/data/opencode-plugin"), false, Some(r#"{"theme":"dark","mcp":{"mine":{"type":"remote"}},"plugins":["mine"]}"#));
        assert_eq!(env[0].0, "OPENCODE_CONFIG_CONTENT");
        let config: Value = serde_json::from_str(&env[0].1).unwrap();
        assert_eq!(config["theme"], "dark");
        assert_eq!(config["mcp"]["mine"]["type"], "remote");
        assert_eq!(config["mcp"]["winfer"]["command"], json!(["/app/winfer", "mcp-stdio"]));
        assert_eq!(config["mcp"]["winfer"]["environment"]["WINFER_PANE_ID"], "p-1");
        assert_eq!(config["plugins"], json!(["mine", "/data/opencode-plugin"]));
        assert!(agent_env("claude", &servers, None, false, None).is_empty());
        let env = agent_env("opencode", &servers, None, false, None);
        let config: Value = serde_json::from_str(&env[0].1).unwrap();
        assert!(config.get("plugins").is_none());
        assert!(config.get("permission").is_none());
    }

    #[test]
    fn opencode_1_auto_approves_through_its_config() {
        let env = agent_env("opencode", &[], None, true, Some(r#"{"permission":{"bash":"ask"}}"#));
        let config: Value = serde_json::from_str(&env[0].1).unwrap();
        assert_eq!(config["permission"], json!({"bash": "ask", "edit": "allow", "webfetch": "allow"}));
        assert!(agent_env("opencode", &[], None, false, None).is_empty());
    }

    #[test]
    fn pi_gets_servers_through_its_extension_env() {
        let servers = [Server {
            name: "winfer",
            command: "/app/winfer".into(),
            args: vec!["mcp-stdio".into()],
            env: vec![("WINFER_PANE_ID", "p-1".into())],
        }];
        assert!(agent_env("pi", &[], None, false, None).is_empty());
        let env = agent_env("pi", &servers, None, false, None);
        assert_eq!(env[0].0, "WINFER_PI_MCP");
        let config: Value = serde_json::from_str(&env[0].1).unwrap();
        assert_eq!(config["winfer"]["command"], "/app/winfer");
        assert_eq!(config["winfer"]["args"], json!(["mcp-stdio"]));
        assert_eq!(config["winfer"]["env"]["WINFER_PANE_ID"], "p-1");
    }

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
                "open_file",
                "notify_user",
                "usage_status",
                "wait_for_usage_reset"
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
    fn hides_tools_from_lists_only() {
        let hidden = ["browser_drag".to_string(), "kb_save".to_string()];
        let mut list = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [{ "name": "browser_click" }, { "name": "browser_drag" }, { "name": "kb_save" }] } });
        assert!(hide_tools(&mut list, &hidden));
        assert_eq!(list["result"]["tools"], json!([{ "name": "browser_click" }]));
        // Nothing to hide, or not a list: left alone.
        assert!(!hide_tools(&mut list, &hidden));
        let mut call = json!({ "jsonrpc": "2.0", "id": 2, "result": { "content": [] } });
        assert!(!hide_tools(&mut call, &hidden));
    }

    #[test]
    fn filters_a_server_through_this_binary() {
        let server = || Server { name: "winfer-browser", command: "cmd".into(), args: vec!["/c".into(), "npx".into()], env: Vec::new() };
        let wrapped = filtered(server(), &["browser_drag".into(), "browser_drop".into()]);
        assert_eq!(wrapped.command, std::env::current_exe().unwrap().to_string_lossy());
        assert_eq!(wrapped.args, ["mcp-filter", "browser_drag,browser_drop", "--", "cmd", "/c", "npx"]);
        // Nothing hidden: started as it is.
        assert_eq!(filtered(server(), &[]).command, "cmd");
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
        assert!(config["mcpServers"]["winfer"].get("alwaysLoad").is_none());
        let cg = [Server { name: CODEGRAPH, command: "/bin/codegraph".into(), args: vec!["serve".into(), "--mcp".into()], env: Vec::new() }];
        let config: Value = serde_json::from_str(&agent_args("claude", &cg)[1]).unwrap();
        assert_eq!(config["mcpServers"]["codegraph"]["alwaysLoad"], true);
    }

    #[test]
    fn finds_the_codegraph_index_up_to_the_repo_root() {
        let dir = std::env::temp_dir().join(format!("winfer-cg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/.codegraph")).unwrap();
        std::fs::create_dir_all(dir.join("repo/src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("other/.git")).unwrap();
        std::fs::write(dir.join("repo/.codegraph/codegraph.db"), "").unwrap();
        assert_eq!(codegraph_root(&dir.join("repo/src/deep")), Some(dir.join("repo")));
        assert_eq!(codegraph_root(&dir.join("repo")), Some(dir.join("repo")));
        assert_eq!(codegraph_root(&dir.join("other")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
