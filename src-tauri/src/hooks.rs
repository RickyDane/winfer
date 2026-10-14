//! Agent status via the agents' own hook systems.
//!
//! A tiny HTTP server on 127.0.0.1 (random port, random token) receives events from
//! Claude Code hooks and the Codex `notify` program, and forwards them to the webview as
//! `agent://status`. PTYs get `WINFER_HOOK` / `WINFER_TOKEN` so the hook commands can reach it.

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
}

static SERVER: OnceLock<HookServer> = OnceLock::new();

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
}

/// Maps a provider hook event to a pane status.
fn status_for(provider: &str, event: &str) -> Option<&'static str> {
    match (provider, event) {
        ("claude", "SessionStart") => Some("idle"),
        ("claude", "UserPromptSubmit" | "PostToolUse") => Some("working"),
        ("claude", "Notification") => Some("needs-input"),
        ("claude", "Stop") => Some("done"),
        ("codex", "turn-complete") => Some("done"),
        _ => None,
    }
}

/// curl ships with macOS and Windows 10+, so hooks need no helper binary.
fn hook_command(provider: &str, event: &str, body_arg: &str) -> String {
    format!(
        "curl -s -m 2 -X POST -H \"X-Winfer-Token: $WINFER_TOKEN\" --data-binary {body_arg} \
         \"http://$WINFER_HOOK/hook/{provider}/$WINFER_PANE_ID/{event}\" >/dev/null 2>&1 || true"
    )
}

fn claude_settings() -> Value {
    let hook = |event: &str| json!([{ "hooks": [{ "type": "command", "command": hook_command("claude", event, "@-") }] }]);
    json!({
        "hooks": {
            "SessionStart": hook("SessionStart"),
            "UserPromptSubmit": hook("UserPromptSubmit"),
            "PostToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": hook_command("claude", "PostToolUse", "@-") }] }],
            "Notification": hook("Notification"),
            "Stop": hook("Stop"),
        }
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
        // Codex runs `notify` with the event JSON as the last argument (through sh, so not on
        // Windows).
        "codex" if cfg!(not(windows)) => {
            let script = hook_command("codex", "turn-complete", "\"$1\"");
            vec![
                "-c".into(),
                format!("notify=['sh','-c','{script}','winfer']"),
            ]
        }
        _ => Vec::new(),
    }
}

pub fn start(app: AppHandle, data_dir: &std::path::Path) -> std::io::Result<()> {
    let settings_path = data_dir.join("claude-hooks.json");
    std::fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&claude_settings())?,
    )?;

    let token = format!("{:032x}", rand::random::<u128>());
    let status_app = app.clone();
    let addr = serve(
        token.clone(),
        move |event| {
            let _ = status_app.emit("agent://status", event);
        },
        move |call| crate::mcp::dispatch(&app, call),
    )?;
    let _ = SERVER.set(HookServer {
        addr,
        token,
        claude_settings: settings_path,
    });
    Ok(())
}

/// Binds 127.0.0.1 on a random port and handles hook requests on a background thread.
/// `/mcp` calls (from `winfer mcp-stdio`) are answered on their own threads since they wait
/// for the webview, so status hooks never queue behind them.
fn serve(
    token: String,
    emit: impl Fn(StatusEvent) + Send + 'static,
    on_mcp: impl Fn(crate::mcp::Call) -> crate::mcp::Reply + Send + Sync + 'static,
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
                    let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
                    if let Some(status) = status_for(provider, event) {
                        let message = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
                            v.get("message").and_then(Value::as_str).map(String::from)
                        });
                        emit(StatusEvent {
                            pane_id: pane_id.clone(),
                            provider: provider.clone(),
                            event: event.clone(),
                            status,
                            message,
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
    fn maps_events_to_statuses() {
        assert_eq!(status_for("claude", "Stop"), Some("done"));
        assert_eq!(status_for("claude", "Notification"), Some("needs-input"));
        assert_eq!(status_for("codex", "turn-complete"), Some("done"));
        assert_eq!(status_for("claude", "Unknown"), None);
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

    /// Runs the real hook commands through `sh` + curl against the server.
    #[cfg(unix)]
    #[test]
    fn hook_commands_reach_the_server() {
        let (tx, rx) = std::sync::mpsc::channel();
        let addr = serve(
            "secret".into(),
            move |e| tx.send(e).unwrap(),
            |call| crate::mcp::Reply {
                ok: true,
                text: format!("{} from {}", call.tool, call.pane_id),
            },
        )
        .unwrap();
        let run = |script: &str, token: &str, args: &[&str]| {
            std::process::Command::new("sh")
                .args(["-c", script, "winfer"])
                .args(args)
                .env("WINFER_HOOK", &addr)
                .env("WINFER_TOKEN", token)
                .env("WINFER_PANE_ID", "p-1")
                .stdin(std::process::Stdio::null())
                .status()
                .unwrap();
        };
        let recv = || rx.recv_timeout(std::time::Duration::from_secs(5));

        // Claude: JSON on stdin.
        let claude = format!(
            "echo '{{\"message\":\"Claude needs your permission\"}}' | {}",
            hook_command("claude", "Notification", "@-")
        );
        run(&claude, "secret", &[]);
        let event = recv().unwrap();
        assert_eq!(
            (event.pane_id.as_str(), event.status),
            ("p-1", "needs-input")
        );
        assert_eq!(
            event.message.as_deref(),
            Some("Claude needs your permission")
        );

        // Codex: JSON as the last argument.
        run(
            &hook_command("codex", "turn-complete", "\"$1\""),
            "secret",
            &[r#"{"type":"agent-turn-complete"}"#],
        );
        assert_eq!(recv().unwrap().status, "done");

        // Wrong token is rejected.
        run(&hook_command("claude", "Stop", "@-"), "wrong", &[]);
        assert!(recv().is_err());
    }

    #[test]
    fn claude_settings_wire_every_event() {
        let settings = claude_settings();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Notification",
            "Stop",
        ] {
            let command = settings["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert!(
                command.contains(&format!("/hook/claude/$WINFER_PANE_ID/{event}")),
                "{event}: {command}"
            );
        }
    }
}
