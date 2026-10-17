//! A small stdio MCP client, so the ChatGPT backend can use MCP servers (winfer's own, the
//! browser, the user's). Tools are exposed to the model as `mcp__<server>__<tool>`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

pub struct Server {
    pub name: String,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next: u64,
    pub tools: Vec<Value>,
}

impl Server {
    pub fn start(name: &str, spec: &Value, cwd: &std::path::Path) -> Result<Server, String> {
        let command = spec["command"].as_str().ok_or("no command")?;
        let program = crate::env::find_command(command).unwrap_or_else(|| command.into());
        let mut cmd = Command::new(program);
        cmd.args(spec["args"].as_array().into_iter().flatten().filter_map(Value::as_str));
        for (k, v) in spec["env"].as_object().into_iter().flatten() {
            cmd.env(k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
        }
        cmd.env("PATH", crate::env::path_var()).current_dir(cwd);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        crate::env::no_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("couldn't start MCP server {name}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let mut server = Server { name: name.to_string(), child, stdin, rx, next: 0, tools: Vec::new() };
        server.call(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "winfer-code", "version": env!("CARGO_PKG_VERSION") } }),
            Duration::from_secs(20),
        )?;
        server.notify("notifications/initialized")?;
        let list = server.call("tools/list", json!({}), Duration::from_secs(20))?;
        server.tools = list["tools"].as_array().cloned().unwrap_or_default();
        Ok(server)
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        let line = json!({ "jsonrpc": "2.0", "method": method }).to_string() + "\n";
        self.stdin.write_all(line.as_bytes()).map_err(|e| e.to_string())
    }

    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string() + "\n";
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(std::time::Instant::now()).ok_or_else(|| format!("{} didn't answer {method}", self.name))?;
            let msg = self.rx.recv_timeout(left).map_err(|_| format!("{} didn't answer {method}", self.name))?;
            if msg["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(err["message"].as_str().unwrap_or("MCP error").to_string());
            }
            return Ok(msg["result"].clone());
        }
    }

    /// Calls a tool; returns (ok, text).
    pub fn call_tool(&mut self, tool: &str, args: Value) -> (bool, String) {
        match self.call("tools/call", json!({ "name": tool, "arguments": args }), Duration::from_secs(600)) {
            Ok(result) => {
                let text = result["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| match c["type"].as_str() {
                        Some("text") => c["text"].as_str().unwrap_or("").to_string(),
                        Some(kind) => format!("[{kind} content]"),
                        None => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (result["isError"].as_bool() != Some(true), text)
            }
            Err(e) => (false, e),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One `mcp` tool for all servers. Its description lists only tool names; schemas are fetched
/// on demand (`mcp(server)`), so dozens of MCP tools cost a line each instead of a full schema.
pub fn meta_tool(servers: &[Server]) -> Option<Value> {
    if servers.is_empty() {
        return None;
    }
    let list: Vec<String> = servers
        .iter()
        .map(|s| format!("{}: {}", s.name, s.tools.iter().filter_map(|t| t["name"].as_str()).collect::<Vec<_>>().join(", ")))
        .collect();
    Some(json!({
        "type": "function",
        "name": "mcp",
        "description": format!("Call an MCP tool; omit tool to get a server's tool schemas first. {}", list.join("; ")),
        "parameters": {
            "type": "object",
            "properties": { "server": { "type": "string" }, "tool": { "type": "string" }, "args": { "type": "object" } },
            "required": ["server"],
        },
    }))
}

impl Server {
    /// Compact schemas: `name(arg: type, opt?: type) - first sentence`.
    pub fn schemas(&self) -> String {
        self.tools
            .iter()
            .map(|t| {
                let schema = &t["inputSchema"];
                let required: Vec<&str> = schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let args: Vec<String> = schema["properties"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        let ty = v["type"].as_str().map(str::to_string).or_else(|| v["enum"].as_array().map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("|"))).unwrap_or_else(|| "any".into());
                        format!("{k}{}: {ty}", if required.contains(&k.as_str()) { "" } else { "?" })
                    })
                    .collect();
                let desc = t["description"].as_str().unwrap_or("");
                let first = desc.split(". ").next().unwrap_or(desc).lines().next().unwrap_or("");
                let first: String = first.chars().take(120).collect();
                format!("{}({}) - {first}", t["name"].as_str().unwrap_or(""), args.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_schemas_and_meta_tool() {
        let (_tx, rx) = channel();
        let child = Command::new("true").stdin(Stdio::piped()).spawn().unwrap();
        let mut server = Server { name: "w".into(), stdin: dummy_stdin(), child, rx, next: 0, tools: Vec::new() };
        server.tools = vec![json!({
            "name": "read_pane",
            "description": "Read a pane. Long details here.",
            "inputSchema": { "type": "object", "properties": { "pane": { "type": "integer" }, "lines": { "type": "integer" } }, "required": ["pane"] },
        })];
        assert_eq!(server.schemas(), "read_pane(lines?: integer, pane: integer) - Read a pane");
        let meta = meta_tool(std::slice::from_ref(&server)).unwrap();
        assert!(meta["description"].as_str().unwrap().ends_with("w: read_pane"));
    }

    fn dummy_stdin() -> ChildStdin {
        Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap().stdin.take().unwrap()
    }
}
