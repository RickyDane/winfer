//! The real `winfer mcp-stdio` binary: MCP over stdio, tool calls forwarded to the app's /mcp.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[test]
fn stdio_server_forwards_tool_calls() {
    // Stand-in for the running app's hook server.
    let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = http.server_addr().to_ip().unwrap().to_string();
    let server = std::thread::spawn(move || {
        let mut req = http.recv().unwrap();
        let token = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-Winfer-Token"))
            .map(|h| h.value.to_string());
        let mut body = String::new();
        req.as_reader().read_to_string(&mut body).unwrap();
        let call: Value = serde_json::from_str(&body).unwrap();
        let text = format!(
            "{}:{}:{}:{}",
            token.unwrap_or_default(),
            call["tool"],
            call["paneId"],
            call["args"]["pane"]
        );
        req.respond(tiny_http::Response::from_string(
            json!({ "ok": true, "text": text }).to_string(),
        ))
        .unwrap();
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_winfer"))
        .arg("mcp-stdio")
        .env("WINFER_HOOK", &addr)
        .env("WINFER_TOKEN", "secret")
        .env("WINFER_PANE_ID", "p-9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for msg in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_pane","arguments":{"pane":2}}}"#,
        ] {
            writeln!(stdin, "{msg}").unwrap();
        }
    }
    drop(child.stdin.take()); // EOF ends the server
    let out = child.wait_with_output().unwrap();
    server.join().unwrap();

    let lines: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        lines.len(),
        3,
        "one response per request, none for the notification"
    );
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "winfer");
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 6);
    assert_eq!(
        lines[2]["result"]["content"][0]["text"],
        r#"secret:"read_pane":"p-9":2"#
    );
    assert_eq!(lines[2]["result"]["isError"], false);
}
