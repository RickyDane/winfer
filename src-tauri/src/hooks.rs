//! Agent status via the agents' own hook systems.
//!
//! A tiny HTTP server on 127.0.0.1 (random port, random token) receives events from
//! Claude Code hooks, the Codex `notify` program and OpenCode 2's plugin
//! (resources/opencode-plugin.js), and forwards them to the webview as
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
    /// The tool that just ran (Claude Code's PostToolUse), and the subagent it handed work to
    /// when that tool was `Agent`; plugins see which work got delegated.
    tool: Option<String>,
    subagent: Option<String>,
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

/// The subagent a Claude Code `Agent` (formerly `Task`) tool call went to; built-in agents
/// without a type count as "general-purpose".
fn subagent_of(json: &Value) -> Option<String> {
    if !matches!(json.get("tool_name")?.as_str()?, "Agent" | "Task") {
        return None;
    }
    let kind = json.get("tool_input").and_then(|i| i.get("subagent_type")).and_then(Value::as_str);
    Some(kind.filter(|k| !k.is_empty()).unwrap_or("general-purpose").to_string())
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

    let token = format!("{:032x}", rand::random::<u128>());
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
                    } else if let Some(status) = status_for(provider, event) {
                        let json = serde_json::from_str::<Value>(&body).ok();
                        if is_reminder(provider, event, json.as_ref()) {
                            let _ = req.respond(tiny_http::Response::empty(204));
                            continue;
                        }
                        let field = |key: &str| json.as_ref().and_then(|v| v.get(key)).and_then(Value::as_str).map(String::from);
                        let message = field("message");
                        let session = field("thread-id").or_else(|| field("session_id"));
                        let reply = if status == "done" { json.as_ref().and_then(last_reply) } else { None };
                        let tool = field("tool_name");
                        let subagent = json.as_ref().and_then(subagent_of);
                        emit(StatusEvent {
                            pane_id: pane_id.clone(),
                            provider: provider.clone(),
                            event: event.clone(),
                            status,
                            message,
                            session,
                            reply,
                            tool,
                            subagent,
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

    #[test]
    fn names_the_subagent_of_agent_calls() {
        let call = |tool: &str, input: Value| json!({ "tool_name": tool, "tool_input": input });
        assert_eq!(subagent_of(&call("Agent", json!({ "subagent_type": "scout" }))).as_deref(), Some("scout"));
        assert_eq!(subagent_of(&call("Task", json!({}))).as_deref(), Some("general-purpose"));
        assert_eq!(subagent_of(&call("Bash", json!({ "subagent_type": "scout" }))), None);
    }

    #[test]
    fn maps_events_to_statuses() {
        assert_eq!(status_for("claude", "Stop"), Some("done"));
        assert_eq!(status_for("claude", "Notification"), Some("needs-input"));
        assert_eq!(status_for("codex", "turn-complete"), Some("done"));
        assert_eq!(status_for("opencode", "permission.asked"), Some("needs-input"));
        assert_eq!(status_for("opencode", "form.created"), Some("needs-input"));
        assert_eq!(status_for("opencode", "session.execution.succeeded"), Some("done"));
        assert_eq!(status_for("claude", "Unknown"), None);
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
