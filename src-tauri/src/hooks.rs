//! Agent status via the agents' own hook systems.
//!
//! A tiny HTTP server on 127.0.0.1 (random port, random token) receives events from
//! Claude Code hooks, the Codex `notify` program, OpenCode 2's plugin
//! (resources/opencode-plugin.js) and pi's extension (resources/pi-extension.js), and forwards them to the webview as
//! `agent://status`. PTYs get `WINFER_HOOK` / `WINFER_TOKEN` so the hook program
//! (`winfer hook`, see [`run_hook`]) can reach it.

use std::io::Read;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

pub struct HookServer {
    pub addr: String,
    pub token: String,
    /// Claude Code settings file with our hooks, passed via `--settings`.
    pub claude_settings: PathBuf,
    /// Directory of the OpenCode plugin, listed in its `plugins` config.
    pub opencode_plugin: PathBuf,
    /// pi extension file, passed via `--extension`; it also registers winfer's MCP servers.
    pub pi_extension: PathBuf,
}

static SERVER: OnceLock<HookServer> = OnceLock::new();

/// Largest hook body read; a Stop event carries the whole final reply.
const MAX_BODY: u64 = 1024 * 1024;

pub fn server() -> Option<&'static HookServer> {
    SERVER.get()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusEvent {
    pane_id: String,
    provider: String,
    event: String,
    status: &'static str,
    message: Option<String>,
    /// The agent's session id, when the event names it (Claude Code's and OpenCode's
    /// `session_id`, Codex's `thread-id`); winfer resumes it after a restart.
    session: Option<String>,
    /// The agent's final reply of the turn, on "done" events that carry it.
    reply: Option<String>,
}

/// Maps a provider hook event to a pane status.
fn status_for(provider: &str, event: &str) -> Option<&'static str> {
    match (provider, event) {
        ("claude", "SessionStart") => Some("idle"),
        ("claude", "UserPromptSubmit" | "PostToolUse") => Some("working"),
        ("claude", "Notification") => Some("needs-input"),
        ("claude", "Stop") => Some("done"),
        ("codex", "turn-complete") => Some("done"),
        ("opencode", "session.execution.started" | "permission.replied" | "form.replied" | "form.cancelled") => {
            Some("working")
        }
        ("opencode", "permission.asked" | "form.created" | "session.execution.failed") => Some("needs-input"),
        ("opencode", "session.execution.succeeded") => Some("done"),
        ("opencode", "session.execution.interrupted") => Some("idle"),
        ("pi", "session_start" | "agent_aborted") => Some("idle"),
        ("pi", "agent_start" | "ui_prompt_end") => Some("working"),
        ("pi", "ui_prompt_start") => Some("needs-input"),
        ("pi", "agent_settled") => Some("done"),
        ("winfer", "session_start" | "interrupted") => Some("idle"),
        ("winfer", "turn_start" | "approved") => Some("working"),
        ("winfer", "needs_input") => Some("needs-input"),
        ("winfer", "turn_done") => Some("done"),
        _ => None,
    }
}

/// Claude Code's `Notification` hook also fires as a reminder once the agent has sat idle
/// after finishing a turn ("Claude is waiting for your input"), and after logging in. Neither
/// is a request: the pane keeps its "done" status instead of turning into "needs input".
fn is_reminder(provider: &str, event: &str, json: Option<&Value>) -> bool {
    if (provider, event) != ("claude", "Notification") {
        return false;
    }
    let field = |key: &str| json.and_then(|v| v.get(key)).and_then(Value::as_str);
    match field("notification_type") {
        Some(kind) => matches!(kind, "idle_prompt" | "auth_success"),
        // Older Claude Code versions send only the message.
        None => field("message").is_some_and(|m| m.contains("waiting for your input")),
    }
}

/// The agent's last reply from a turn-complete event: Claude Code's `last_assistant_message`
/// (else the last assistant text in its transcript), Codex's `last-assistant-message`.
fn last_reply(json: &Value) -> Option<String> {
    let text = |key: &str| {
        let text = json.get(key)?.as_str()?.trim();
        (!text.is_empty()).then(|| text.to_string())
    };
    text("last_assistant_message")
        .or_else(|| text("last-assistant-message"))
        .or_else(|| transcript_reply(json.get("transcript_path")?.as_str()?))
}

/// The text of the last assistant message in a Claude Code transcript (JSONL), from the
/// current turn only: the search stops at the user's prompt.
fn transcript_reply(path: &str) -> Option<String> {
    let file = std::fs::read_to_string(path).ok()?;
    for entry in file.lines().rev().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        let content = &entry["message"]["content"];
        let texts = || {
            content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["type"] == "text")
                .filter_map(|c| c["text"].as_str())
        };
        match entry["type"].as_str() {
            Some("assistant") => {
                let text = texts().collect::<Vec<_>>().join("\n\n").trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
            // Tool results come back as user entries too; a prompt has text.
            Some("user") if content.is_string() || texts().next().is_some() => return None,
            _ => {}
        }
    }
    None
}

/// Background work started within this long before a stop still counts as running; older
/// launches without a completion are taken for orphans (their Claude Code process exited).
const BACKGROUND_MAX_AGE_SECS: u64 = 12 * 3600;

/// Whether a Claude Code session still has background work running (shell commands run in the
/// background, async subagents): its `Stop` then only ends the current turn, as each task's
/// completion wakes the agent again. Launches are tool results carrying a `backgroundTaskId`,
/// or an `agentId` with status `async_launched`; completions are `<task-notification>`s.
fn background_pending(path: &str) -> bool {
    let Ok(file) = std::fs::read_to_string(path) else {
        return false;
    };
    let cutoff = iso_utc(std::time::SystemTime::now(), BACKGROUND_MAX_AGE_SECS);
    let mut launched: Vec<String> = Vec::new();
    let mut finished: Vec<String> = Vec::new();
    for line in file.lines() {
        if line.contains("<task-notification>") {
            let mut rest = line;
            while let Some(start) = rest.find("<task-id>") {
                rest = &rest[start + "<task-id>".len()..];
                let Some(end) = rest.find("</task-id>") else { break };
                finished.push(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
        if !line.contains("backgroundTaskId") && !line.contains("async_launched") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        if entry["timestamp"].as_str().is_some_and(|t| t < cutoff.as_str()) {
            continue;
        }
        let result = &entry["toolUseResult"];
        let id = result["backgroundTaskId"]
            .as_str()
            .or_else(|| (result["status"] == "async_launched").then(|| result["agentId"].as_str()).flatten());
        if let Some(id) = id {
            launched.push(id.to_string());
        }
    }
    launched.iter().any(|id| !finished.contains(id))
}

/// `now - age_secs` as an RFC 3339 UTC timestamp (`2026-10-20T19:43:24Z`), comparable as a
/// string with the timestamps in Claude Code's transcripts.
fn iso_utc(now: std::time::SystemTime, age_secs: u64) -> String {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        .saturating_sub(age_secs);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The hook program: `winfer hook <provider> <event>`, run without a shell (Claude Code's
/// exec form, Codex's `notify` argv). A shell command wouldn't work everywhere: on Windows,
/// Claude Code runs hooks in PowerShell when Git Bash isn't installed, and Codex uses no shell.
fn hook_argv(exe: &str, provider: &str, event: &str) -> Vec<String> {
    vec![exe.into(), "hook".into(), provider.into(), event.into()]
}

fn current_exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "winfer".into())
}

fn claude_settings(exe: &str) -> Value {
    let command = |event: &str| {
        let argv = hook_argv(exe, "claude", event);
        json!({ "type": "command", "command": argv[0], "args": argv[1..] })
    };
    let hook = |event: &str| json!([{ "hooks": [command(event)] }]);
    json!({
        "hooks": {
            "SessionStart": hook("SessionStart"),
            "UserPromptSubmit": hook("UserPromptSubmit"),
            "PostToolUse": [{ "matcher": "*", "hooks": [command("PostToolUse")] }],
            "Notification": hook("Notification"),
            "Stop": hook("Stop"),
        },
        // Reports the plan's usage limits, then shows the user's own status line.
        "statusLine": crate::usage::claude_statusline_setting(exe),
    })
}

/// Extra CLI arguments that wire a provider's hooks to the server.
pub fn provider_args(provider_id: &str) -> Vec<String> {
    let Some(server) = server() else {
        return Vec::new();
    };
    match provider_id {
        "claude" => vec![
            "--settings".into(),
            server.claude_settings.to_string_lossy().into_owned(),
        ],
        // Codex runs `notify` with the event JSON appended as the last argument.
        "codex" => {
            let argv: Vec<String> = hook_argv(&current_exe(), "codex", "turn-complete")
                .iter()
                .map(|a| toml_string(a))
                .collect();
            vec!["-c".into(), format!("notify=[{}]", argv.join(","))]
        }
        "pi" => vec![
            "--extension".into(),
            server.pi_extension.to_string_lossy().into_owned(),
        ],
        _ => Vec::new(),
    }
}

/// A TOML basic string (Windows paths are full of backslashes).
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Entry point for `winfer hook <provider> <event> [json]`: forwards the event JSON (the
/// last argument, else stdin) to the running app. Always succeeds so it never blocks an agent.
pub fn run_hook(args: &[String]) {
    let [provider, event, rest @ ..] = args else {
        return;
    };
    let body = match rest.last() {
        Some(json) => json.clone(),
        None => {
            let mut body = String::new();
            let _ = std::io::stdin().take(MAX_BODY).read_to_string(&mut body);
            body
        }
    };
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    send_hook(
        &var("WINFER_HOOK"),
        &var("WINFER_TOKEN"),
        &var("WINFER_PANE_ID"),
        provider,
        event,
        body,
    );
}

pub fn send_hook(hook: &str, token: &str, pane_id: &str, provider: &str, event: &str, body: String) {
    if hook.is_empty() || pane_id.is_empty() {
        return;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(2)))
        .build()
        .into();
    let _ = agent
        .post(&format!("http://{hook}/hook/{provider}/{pane_id}/{event}"))
        .header("X-Winfer-Token", token)
        .send(body);
}

pub fn start(app: AppHandle, data_dir: &std::path::Path) -> std::io::Result<()> {
    let settings_path = data_dir.join("claude-hooks.json");
    std::fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&claude_settings(&current_exe()))?,
    )?;
    let opencode_plugin = data_dir.join("opencode-plugin");
    write_opencode_plugin(&opencode_plugin)?;
    let pi_extension = data_dir.join("pi-extension.js");
    std::fs::write(&pi_extension, include_str!("../resources/pi-extension.js"))?;

    let token = format!("{:032x}", rand::random::<u128>());
    crate::mcp::listen_bg(app.clone());
    let status_app = app.clone();
    let usage_app = app.clone();
    let addr = serve(
        token.clone(),
        move |event| {
            let _ = status_app.emit("agent://status", event);
        },
        move |call| crate::mcp::dispatch(&app, call),
        move |provider, pane_id, body| {
            let _ = usage_app.emit("usage://report", json!({ "provider": provider, "paneId": pane_id, "report": body }));
        },
    )?;
    let _ = SERVER.set(HookServer {
        addr,
        token,
        claude_settings: settings_path,
        opencode_plugin,
        pi_extension,
    });
    Ok(())
}

/// OpenCode 2 loads a plugin from a directory: a package whose entry point is the plugin.
fn write_opencode_plugin(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let package = json!({ "name": "winfer-status", "private": true, "type": "module", "exports": { ".": "./index.js" } });
    std::fs::write(dir.join("package.json"), serde_json::to_vec_pretty(&package)?)?;
    std::fs::write(dir.join("index.js"), include_str!("../resources/opencode-plugin.js"))
}

/// Binds 127.0.0.1 on a random port and handles hook requests on a background thread.
/// `/mcp` calls (from `winfer mcp-stdio`) are answered on their own threads since they wait
/// for the webview, so status hooks never queue behind them.
fn serve(
    token: String,
    emit: impl Fn(StatusEvent) + Send + 'static,
    on_mcp: impl Fn(crate::mcp::Call) -> crate::mcp::Reply + Send + Sync + 'static,
    on_usage: impl Fn(&str, &str, Value) + Send + 'static,
) -> std::io::Result<String> {
    let on_mcp = std::sync::Arc::new(on_mcp);
    let http = tiny_http::Server::http("127.0.0.1:0").map_err(std::io::Error::other)?;
    let addr = http
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .ok_or_else(|| std::io::Error::other("hook server has no IP address"))?;

    std::thread::spawn(move || {
        for mut req in http.incoming_requests() {
            let authorized = req
                .headers()
                .iter()
                .any(|h| h.field.equiv("X-Winfer-Token") && h.value.as_str() == token);
            let parts: Vec<String> = req
                .url()
                .trim_start_matches('/')
                .split('/')
                .map(String::from)
                .collect();

            let code = match (authorized, parts.as_slice()) {
                (false, _) => 401,
                (true, [hook, provider, pane_id, event]) if hook == "hook" => {
                    let mut body = String::new();
                    let _ = req.as_reader().take(MAX_BODY).read_to_string(&mut body);
                    if event == "usage" {
                        if let Ok(report) = serde_json::from_str::<Value>(&body) {
                            on_usage(provider, pane_id, report);
                        }
                    } else if let Some(mut status) = status_for(provider, event) {
                        let json = serde_json::from_str::<Value>(&body).ok();
                        if is_reminder(provider, event, json.as_ref()) {
                            let _ = req.respond(tiny_http::Response::empty(204));
                            continue;
                        }
                        // A turn that ends while background tasks or subagents still run isn't
                        // the end of the work: their results start another turn.
                        if (provider.as_str(), event.as_str()) == ("claude", "Stop")
                            && json
                                .as_ref()
                                .and_then(|v| v["transcript_path"].as_str())
                                .is_some_and(background_pending)
                        {
                            status = "working";
                        }
                        let field = |key: &str| json.as_ref().and_then(|v| v.get(key)).and_then(Value::as_str).map(String::from);
                        let message = field("message");
                        let session = field("thread-id").or_else(|| field("session_id"));
                        let reply = if status == "done" { json.as_ref().and_then(last_reply) } else { None };
                        emit(StatusEvent {
                            pane_id: pane_id.clone(),
                            provider: provider.clone(),
                            event: event.clone(),
                            status,
                            message,
                            session,
                            reply,
                        });
                    }
                    204
                }
                (true, [mcp]) if mcp == "mcp" => {
                    let on_mcp = on_mcp.clone();
                    std::thread::spawn(move || {
                        let mut body = String::new();
                        let _ = req.as_reader().take(1024 * 1024).read_to_string(&mut body);
                        let reply = match serde_json::from_str::<crate::mcp::Call>(&body) {
                            Ok(call) => on_mcp(call),
                            Err(err) => crate::mcp::Reply {
                                ok: false,
                                text: format!("bad request: {err}"),
                            },
                        };
                        let json = serde_json::to_string(&reply).unwrap_or_default();
                        let header =
                            tiny_http::Header::from_bytes("Content-Type", "application/json")
                                .expect("valid header");
                        let _ =
                            req.respond(tiny_http::Response::from_string(json).with_header(header));
                    });
                    continue;
                }
                _ => 404,
            };
            let _ = req.respond(tiny_http::Response::empty(code));
        }
    });
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole path a pane agent's `bg` call takes: `winfer mcp-stdio` (the built binary) →
    /// the hook server → `bg` in the app. With BG_E2E_CLAUDE=1 a real Claude Code agent drives it.
    /// cargo build && cargo test --lib bg_over_mcp_stdio -- --ignored --nocapture
    #[cfg(unix)]
    #[test]
    #[ignore = "needs the built winfer binary"]
    fn bg_over_mcp_stdio_end_to_end() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        let token = "t0k".to_string();
        static SEEN: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
        let addr = serve(
            token.clone(),
            |_| {},
            |call| {
                if call.tool != "bg" {
                    return crate::mcp::Reply { ok: false, text: "app-only tool".into() };
                }
                SEEN.lock().unwrap().push((call.pane_id.clone(), call.args["action"].as_str().unwrap_or("").to_string()));
                crate::mcp::bg_call(&call)
            },
            |_, _, _| {},
        )
        .unwrap();
        let used = |pane: &str, action: &str| SEEN.lock().unwrap().iter().any(|(p, a)| p == pane && a == action);
        let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/winfer");
        let dir = std::env::temp_dir().join(format!("winfer-bg-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("server.sh"), "echo building; sleep 1; echo 'ready on :4321'; while :; do echo tick; sleep 0.2; done\n").unwrap();
        let mut child = Command::new(&bin)
            .arg("mcp-stdio")
            .env("WINFER_HOOK", &addr).env("WINFER_TOKEN", &token).env("WINFER_PANE_ID", "p-e2e")
            .current_dir(&dir)
            .stdin(Stdio::piped()).stdout(Stdio::piped())
            .spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut rpc = |id: u64, method: &str, params: Value| -> Value {
            writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).unwrap();
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        };
        let init = rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
        assert!(init["result"]["instructions"].as_str().unwrap().contains("bg"));
        let tools = rpc(2, "tools/list", json!({}));
        assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "bg"));
        let text = |v: &Value| v["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
        let start = rpc(3, "tools/call", json!({ "name": "bg", "arguments": { "action": "start", "cmd": "sh server.sh", "until": "ready" } }));
        let out = text(&start);
        assert!(out.contains("ready on :4321"), "{out}");
        let id = out.split_whitespace().nth(1).unwrap().to_string();
        let read = text(&rpc(4, "tools/call", json!({ "name": "bg", "arguments": { "action": "read", "id": id, "wait": 500 } })));
        assert!(read.contains("tick") && !read.contains("building"), "{read}");
        let stop = text(&rpc(5, "tools/call", json!({ "name": "bg", "arguments": { "action": "stop", "id": id } })));
        assert!(stop.contains("stopped"), "{stop}");
        drop(rpc);
        // Calls sent side by side run side by side: a quick start isn't stuck behind a slow one.
        for (id, cmd) in [(6, "sleep 3; echo slow-ready; sleep 30"), (7, "echo fast-ready; sleep 30")] {
            writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": "bg", "arguments": { "action": "start", "cmd": cmd, "until": "ready" } } })).unwrap();
        }
        let begun = std::time::Instant::now();
        let mut replies = Vec::new();
        for _ in 0..2 {
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            replies.push((serde_json::from_str::<Value>(&line).unwrap(), begun.elapsed()));
        }
        assert_eq!(replies[0].0["id"], 7, "the quick start should answer first");
        assert!(replies[0].1 < std::time::Duration::from_secs(2), "{:?}", replies[0].1);
        assert_eq!(replies[1].0["id"], 6);
        for (reply, _) in &replies {
            let id = text(reply).split_whitespace().nth(1).unwrap().to_string();
            crate::harness::bg::stop(&id);
        }
        drop(stdin);
        let _ = child.wait();
        println!("mcp-stdio bg: ok");

        if std::env::var_os("BG_E2E_CLAUDE").is_some() {
            let config = json!({ "mcpServers": { "winfer": { "command": bin, "args": ["mcp-stdio"], "env": { "WINFER_HOOK": addr, "WINFER_TOKEN": token, "WINFER_PANE_ID": "p-claude" } } } });
            let out = Command::new("claude")
                .args(std::env::var("BG_E2E_PROMPT").ok().map(|p| vec!["--append-system-prompt".to_string(), p]).unwrap_or_default())
                .args(["-p", "--model", "haiku", "--strict-mcp-config", "--mcp-config", &config.to_string(), "--allowedTools", "mcp__winfer__bg,Bash(curl:*)", "--output-format", "stream-json", "--verbose"])
                .arg("Start the dev server `sh server.sh` (it prints 'ready on :4321' when up) in the background, wait until it is ready, tell me the port, then stop it.")
                .current_dir(&dir)
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&out.stdout);
            let calls: Vec<String> = log.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|v| v["message"]["content"].as_array().cloned().unwrap_or_default()).filter(|b| b["type"] == "tool_use").map(|b| format!("{} {}", b["name"].as_str().unwrap_or(""), b["input"])).collect();
            println!("claude tool calls:\n  {}", calls.join("\n  "));
            assert!(calls.iter().any(|c| c.starts_with("mcp__winfer__bg") && c.contains("start")), "claude didn't start it with bg");
            assert!(calls.iter().any(|c| c.starts_with("mcp__winfer__bg") && c.contains("stop")), "claude didn't stop it with bg");
            let left = crate::harness::bg::run(&json!({ "action": "list" }), &dir, &std::sync::atomic::AtomicBool::new(false), "", "p-claude");
            assert!(left.output.contains("no background tasks"), "{}", left.output);
        }
        if std::env::var_os("BG_E2E_CODEX").is_some() {
            let out = Command::new("codex")
                .args(["exec", "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox", "--json"])
                .args(["-c", &format!("mcp_servers.winfer.command={}", json!(bin))])
                .args(["-c", r#"mcp_servers.winfer.args=["mcp-stdio"]"#])
                // What winfer passes every Codex pane (its default agent prompt), when given.
                .args(std::env::var("BG_E2E_PROMPT").ok().map(|p| vec!["-c".to_string(), format!("developer_instructions={}", json!(p))]).unwrap_or_default())
                .args(["-c", &format!(r#"mcp_servers.winfer.env={{WINFER_HOOK="{addr}", WINFER_TOKEN="{token}", WINFER_PANE_ID="p-codex"}}"#)])
                .arg("Start the dev server `sh server.sh` (it prints 'ready on :4321' when up) in the background, wait until it is ready, tell me the port, then stop it.")
                .current_dir(&dir)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let seen: Vec<_> = SEEN.lock().unwrap().iter().filter(|(p, _)| p == "p-codex").cloned().collect();
            println!("codex bg calls: {seen:?}");
            if seen.is_empty() {
                println!("{}", String::from_utf8_lossy(&out.stdout));
            }
            assert!(used("p-codex", "start") && used("p-codex", "stop"), "codex didn't use bg");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn maps_events_to_statuses() {
        assert_eq!(status_for("claude", "Stop"), Some("done"));
        assert_eq!(status_for("claude", "Notification"), Some("needs-input"));
        assert_eq!(status_for("codex", "turn-complete"), Some("done"));
        assert_eq!(status_for("opencode", "permission.asked"), Some("needs-input"));
        assert_eq!(status_for("opencode", "form.created"), Some("needs-input"));
        assert_eq!(status_for("opencode", "session.execution.succeeded"), Some("done"));
        assert_eq!(status_for("pi", "agent_start"), Some("working"));
        assert_eq!(status_for("pi", "ui_prompt_start"), Some("needs-input"));
        assert_eq!(status_for("pi", "agent_settled"), Some("done"));
        assert_eq!(status_for("pi", "agent_aborted"), Some("idle"));
        assert_eq!(status_for("claude", "Unknown"), None);
    }

    #[test]
    fn iso_utc_formats_civil_dates() {
        let at = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        assert_eq!(iso_utc(at(0), 0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(at(1_803_065_004), 0), "2027-02-19T19:23:24Z");
        assert_eq!(iso_utc(at(1_709_208_000 + 3600), 3600), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn background_work_keeps_the_turn_open() {
        let now = iso_utc(std::time::SystemTime::now(), 0);
        let bash = json!({"type":"user","timestamp":now,"toolUseResult":{"stdout":"","backgroundTaskId":"b1"}});
        let agent = json!({"type":"user","timestamp":now,"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"a1"}});
        let old = json!({"type":"user","timestamp":"2020-01-01T00:00:00.000Z","toolUseResult":{"backgroundTaskId":"b0"}});
        let note = |id: &str| json!({"type":"queue-operation","content":format!("<task-notification>\n<task-id>{id}</task-id>\n<status>completed</status>\n</task-notification>")});
        let path = std::env::temp_dir().join(format!("winfer-transcript-{:x}.jsonl", rand::random::<u64>()));
        let check = |entries: &[&Value]| {
            let text = entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
            std::fs::write(&path, text).unwrap();
            background_pending(path.to_str().unwrap())
        };
        assert!(!check(&[&old]));
        assert!(check(&[&bash, &agent, &note("b1")]));
        assert!(!check(&[&bash, &agent, &note("b1"), &note("a1")]));
        let _ = std::fs::remove_file(&path);
        assert!(!background_pending(path.to_str().unwrap()));
    }

    #[test]
    fn idle_reminders_are_not_requests() {
        let idle = json!({"notification_type":"idle_prompt","message":"Claude is waiting for your input"});
        let permission = json!({"notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"});
        let legacy = json!({"message":"Claude is waiting for your input"});
        assert!(is_reminder("claude", "Notification", Some(&idle)));
        assert!(is_reminder("claude", "Notification", Some(&legacy)));
        assert!(!is_reminder("claude", "Notification", Some(&permission)));
        assert!(!is_reminder("claude", "Stop", Some(&idle)));
    }

    #[test]
    fn mcp_route_needs_token_and_forwards_calls() {
        let addr = serve(
            "secret".into(),
            |_| {},
            |call| crate::mcp::Reply {
                ok: true,
                text: format!("{}:{}", call.tool, call.pane_id),
            },
            |_, _, _| {},
        )
        .unwrap();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let body = serde_json::json!({ "tool": "list_panes", "args": {}, "paneId": "p-1" });

        let mut ok = agent
            .post(&format!("http://{addr}/mcp"))
            .header("X-Winfer-Token", "secret")
            .send_json(&body)
            .unwrap();
        let reply: Value = ok.body_mut().read_json().unwrap();
        assert_eq!(reply["text"], "list_panes:p-1");

        let denied = agent
            .post(&format!("http://{addr}/mcp"))
            .header("X-Winfer-Token", "nope")
            .send_json(&body)
            .unwrap();
        assert_eq!(denied.status(), 401);
    }

    /// Sends hook events the way `winfer hook` does.
    #[test]
    fn hooks_reach_the_server() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (usage_tx, usage_rx) = std::sync::mpsc::channel();
        let addr = serve(
            "secret".into(),
            move |e| tx.send(e).unwrap(),
            |call| crate::mcp::Reply {
                ok: true,
                text: format!("{} from {}", call.tool, call.pane_id),
            },
            move |provider, _, report| usage_tx.send((provider.to_string(), report)).unwrap(),
        )
        .unwrap();
        let recv = || rx.recv_timeout(std::time::Duration::from_secs(5));

        send_hook(&addr, "secret", "p-1", "claude", "usage", r#"{"windows":[]}"#.into());
        let (provider, report) = usage_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!((provider.as_str(), &report["windows"]), ("claude", &json!([])));

        let body = r#"{"message":"Claude needs your permission"}"#.to_string();
        send_hook(&addr, "secret", "p-1", "claude", "Notification", body);
        let event = recv().unwrap();
        assert_eq!(
            (event.pane_id.as_str(), event.status),
            ("p-1", "needs-input")
        );
        assert_eq!(
            event.message.as_deref(),
            Some("Claude needs your permission")
        );

        let body = r#"{"type":"agent-turn-complete","last-assistant-message":"All done."}"#.to_string();
        send_hook(&addr, "secret", "p-1", "codex", "turn-complete", body);
        let event = recv().unwrap();
        assert_eq!((event.status, event.reply.as_deref()), ("done", Some("All done.")));

        // Wrong token is rejected.
        send_hook(&addr, "wrong", "p-1", "claude", "Stop", String::new());
        assert!(recv().is_err());
    }

    #[test]
    fn last_reply_falls_back_to_the_transcript() {
        let dir = std::env::temp_dir().join(format!("winfer-hooks-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let reply = || last_reply(&json!({ "transcript_path": path.to_string_lossy() }));
        let transcript = [
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Older" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Fixed it." }, { "type": "tool_use" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use" }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result" }] } }),
        ]
        .map(|l| l.to_string())
        .join("\n");
        std::fs::write(&path, &transcript).unwrap();
        assert_eq!(reply().as_deref(), Some("Fixed it."));
        // A new prompt with no reply yet: nothing from the earlier turn.
        std::fs::write(&path, format!("{transcript}\n{}", json!({ "type": "user", "message": { "content": "thanks" } }))).unwrap();
        assert_eq!(reply(), None);
        assert_eq!(last_reply(&json!({ "last_assistant_message": " Hi " })).as_deref(), Some("Hi"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn claude_settings_run_winfer_without_a_shell() {
        let exe = r"C:\Program Files\winfer\winfer.exe";
        let settings = claude_settings(exe);
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Notification",
            "Stop",
        ] {
            let hook = &settings["hooks"][event][0]["hooks"][0];
            assert_eq!(hook["command"], exe, "{event}");
            assert_eq!(hook["args"], json!(["hook", "claude", event]), "{event}");
        }
    }

    #[test]
    fn toml_strings_keep_windows_paths() {
        assert_eq!(
            toml_string(r#"C:\a "b"\winfer.exe"#),
            r#""C:\\a \"b\"\\winfer.exe""#
        );
    }
}
