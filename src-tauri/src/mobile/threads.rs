//! A pane's conversation as chat messages for the phone: the user's prompts, the agent's
//! replies and one-line summaries of its tool calls, read from the agent's own session
//! (Claude Code's transcript, Codex's session log, OpenCode's database, winfer code's saved
//! session).

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::context_breakdown::{claude_injected, codex_injected, tool_label};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub role: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub ts: Option<i64>,
    /// Tools: "running" | "done" | "failed", when the session says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,
    /// Tools: a background shell or subagent, which outlives its call.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

/// Replies longer than this are cut (phones get the gist; the desktop has the rest).
const MAX_TEXT: usize = 16_000;
const MAX_SUMMARY: usize = 160;

fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Milliseconds since the epoch of an RFC 3339 UTC time ("2026-10-19T17:09:57.903Z").
fn parse_ts(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| ts.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let ms = match ts.get(19..20) {
        Some(".") => {
            let frac: String = ts[20..].chars().take_while(char::is_ascii_digit).take(3).collect();
            format!("{frac:0<3}").parse::<i64>().ok()?
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000) + ms)
}

/// "Bash: npm test" from a tool's name and input.
fn summary(name: &str, input: &Value) -> String {
    let input = match input {
        // Codex passes function arguments as a JSON string.
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        v => v.clone(),
    };
    let detail = ["command", "cmd", "file_path", "path", "pattern", "url", "query", "description", "prompt", "skill"]
        .iter()
        .find_map(|k| match &input[k] {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
            _ => None,
        })
        .or_else(|| input.as_str().map(String::from));
    let label = tool_label(name);
    match detail {
        Some(d) => cut(&format!("{label}: {}", d.lines().next().unwrap_or_default()), MAX_SUMMARY),
        None => label,
    }
}

fn push(out: &mut Vec<Message>, id: String, role: &'static str, text: &str, tool: Option<String>, ts: Option<i64>) {
    let text = if role == "tool" { text.to_string() } else { cut(text, MAX_TEXT) };
    if text.is_empty() {
        return;
    }
    // A reply streamed as several entries reads as one message.
    if role == "assistant"
        && let Some(last) = out.last_mut().filter(|m| m.role == "assistant")
    {
        last.text = cut(&format!("{}\n\n{text}", last.text), MAX_TEXT);
        return;
    }
    out.push(Message { id, role, text, tool, ts, state: None, background: false });
}

/// Pushes a tool call and returns its index (None when it has nothing to show).
fn push_tool(out: &mut Vec<Message>, id: String, text: &str, tool: Option<String>, ts: Option<i64>, state: Option<&'static str>) -> Option<usize> {
    let before = out.len();
    push(out, id, "tool", text, tool, ts);
    (out.len() > before).then(|| {
        out[before].state = state;
        before
    })
}

/// The value of `<tag>…</tag>` in `text`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>"))? + start;
    Some(text[start..end].trim())
}

/// Claude Code's word on a background shell or agent it started: the id it reports it by.
fn claude_background_id(result: &str) -> Option<String> {
    let id = |rest: &str| rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-').next().filter(|s| !s.is_empty()).map(String::from);
    if let Some(rest) = result.split("running in background with ID: ").nth(1) {
        return id(rest);
    }
    if result.contains("launched successfully") {
        return id(result.split("agentId: ").nth(1)?);
    }
    None
}

/// `<task-notification>` blocks: (task id, tool-use id, finished state).
fn claude_notifications(text: &str) -> Vec<(Option<String>, Option<String>, &'static str)> {
    text.split("<task-notification>")
        .skip(1)
        .filter_map(|block| {
            let status = tag(block, "status")?;
            let state = match status {
                "completed" | "done" | "success" => "done",
                "running" | "pending" => return None,
                _ => "failed",
            };
            Some((tag(block, "task-id").map(String::from), tag(block, "tool-use-id").map(String::from), state))
        })
        .collect()
}

/// From a Claude Code transcript (JSONL).
pub fn claude(transcript: &str) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    // Tool calls by their id and background tasks by theirs, to mark them done later.
    let mut by_call: HashMap<String, usize> = HashMap::new();
    let mut by_task: HashMap<String, usize> = HashMap::new();
    // Whether the newest entry is the agent reasoning (nothing has followed it yet).
    let mut thinking = false;
    for (n, line) in transcript.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        let ts = v["timestamp"].as_str().and_then(parse_ts);
        let uuid = v["uuid"].as_str().map_or_else(|| n.to_string(), String::from);
        let content = &v["message"]["content"];
        match v["type"].as_str().unwrap_or_default() {
            "user" => {
                thinking = false;
                let texts: Vec<&str> = match content {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect(),
                    _ => vec![],
                };
                // Background work reporting back (not the user typing).
                for t in &texts {
                    for (task, call, state) in claude_notifications(t) {
                        let i = task.and_then(|t| by_task.get(&t).copied()).or_else(|| call.and_then(|c| by_call.get(&c).copied()));
                        if let Some(i) = i {
                            out[i].state = Some(state);
                        }
                    }
                }
                // A call's result: done, failed, or "now running in the background".
                for b in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let Some(&i) = b["tool_use_id"].as_str().and_then(|id| by_call.get(id)) else { continue };
                    let result = match &b["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    if b["is_error"].as_bool() == Some(true) {
                        out[i].state = Some("failed");
                    } else if let Some(task) = claude_background_id(&result) {
                        out[i].background = true;
                        out[i].state = Some("running");
                        by_task.insert(task, i);
                    } else {
                        out[i].state = Some("done");
                    }
                }
                let text = texts.into_iter().filter(|t| !claude_injected(t) && !t.trim_start().starts_with("<task-notification>")).collect::<Vec<_>>().join("\n\n");
                push(&mut out, uuid, "user", &text, None, ts);
            }
            "assistant" => {
                for (i, block) in content.as_array().into_iter().flatten().enumerate() {
                    let id = format!("{uuid}:{i}");
                    let kind = block["type"].as_str().unwrap_or_default();
                    thinking = matches!(kind, "thinking" | "redacted_thinking");
                    match kind {
                        "text" => push(&mut out, id, "assistant", block["text"].as_str().unwrap_or_default(), None, ts),
                        "tool_use" => {
                            let name = block["name"].as_str().unwrap_or("tool");
                            if let Some(i) = push_tool(&mut out, id, &summary(name, &block["input"]), Some(tool_label(name)), ts, Some("running"))
                                && let Some(call) = block["id"].as_str()
                            {
                                by_call.insert(call.to_string(), i);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    mark_thinking(&mut out, thinking);
    out
}

/// Ends the thread with a "thinking" marker when the agent is reasoning right now, so the phone
/// says "Thinking…" only then (the marker has no text and is never shown as a message).
fn mark_thinking(out: &mut Vec<Message>, thinking: bool) {
    if thinking {
        out.push(Message { id: "thinking".into(), role: "thinking", text: String::new(), tool: None, ts: None, state: None, background: false });
    }
}

/// From a Codex session log (JSONL).
pub fn codex(log: &str) -> Vec<Message> {
    let mut out = Vec::new();
    let mut by_call: HashMap<String, usize> = HashMap::new();
    let mut thinking = false;
    for (n, line) in log.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["type"] != "response_item" {
            continue;
        }
        let item = &v["payload"];
        thinking = item["type"] == "reasoning";
        let ts = v["timestamp"].as_str().and_then(parse_ts);
        let id = item["id"].as_str().or(item["call_id"].as_str()).map_or_else(|| n.to_string(), String::from);
        let kind = item["type"].as_str().unwrap_or(if item["role"].is_string() { "message" } else { "" });
        match kind {
            "message" => {
                let text = item["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n\n");
                match item["role"].as_str().unwrap_or_default() {
                    "user" if !codex_injected(&text) => push(&mut out, id, "user", &text, None, ts),
                    "assistant" => push(&mut out, id, "assistant", &text, None, ts),
                    _ => {}
                }
            }
            "local_shell_call" => {
                let state = if item["status"] == "in_progress" { "running" } else { "done" };
                push_tool(&mut out, id, &summary("shell", &item["action"]), Some("shell".into()), ts, Some(state));
            }
            k if k.ends_with("_call_output") => {
                if let Some(&i) = item["call_id"].as_str().and_then(|c| by_call.get(c)) {
                    out[i].state = Some("done");
                }
            }
            k if k.ends_with("_call") => {
                let name = item["name"].as_str().unwrap_or_else(|| k.trim_end_matches("_call"));
                let input = if item["arguments"].is_null() { &item["input"] } else { &item["arguments"] };
                if let (Some(i), Some(call)) = (push_tool(&mut out, id, &summary(name, input), Some(tool_label(name)), ts, Some("running")), item["call_id"].as_str()) {
                    by_call.insert(call.to_string(), i);
                }
            }
            _ => {}
        }
    }
    mark_thinking(&mut out, thinking);
    out
}

/// From an OpenCode session's messages.
pub fn opencode(conn: &rusqlite::Connection, session: &str) -> Option<Vec<Message>> {
    let mut stmt = conn.prepare("select id, type, data from session_message where session_id = ?1 order by seq").ok()?;
    let rows = stmt
        .query_map([session], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .ok()?;
    let mut out = Vec::new();
    for (id, kind, raw) in rows.flatten() {
        let Ok(data) = serde_json::from_str::<Value>(&raw) else { continue };
        let ts = data["time"]["created"].as_i64();
        match kind.as_str() {
            "user" => push(&mut out, id, "user", data["text"].as_str().unwrap_or_default(), None, ts),
            "assistant" => {
                for (i, part) in data["content"].as_array().into_iter().flatten().enumerate() {
                    let id = format!("{id}:{i}");
                    match part["type"].as_str().unwrap_or_default() {
                        "reasoning" => {}
                        "tool" => {
                            let name = part["name"].as_str().unwrap_or("tool");
                            let state = match part["state"]["status"].as_str() {
                                Some("running" | "pending") => Some("running"),
                                Some("completed") => Some("done"),
                                Some("error") => Some("failed"),
                                _ => None,
                            };
                            push_tool(&mut out, id, &summary(name, &part["state"]["input"]), Some(tool_label(name)), ts, state);
                        }
                        _ => push(&mut out, id, "assistant", part["text"].as_str().unwrap_or_default(), None, ts),
                    }
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// From a saved `winfer code` session (`harness::session`), saved as each turn starts, after
/// each tool and when it ends. Notices (compaction, errors) are winfer's, not the chat's.
pub fn winfer(saved: &crate::harness::session::Saved) -> Vec<Message> {
    let mut out = Vec::new();
    for (i, entry) in saved.transcript.iter().enumerate() {
        let id = format!("w{i}");
        match entry.role.as_str() {
            "user" => push(&mut out, id, "user", &entry.text, None, None),
            "assistant" => push(&mut out, id, "assistant", &entry.text, None, None),
            "tool" => {
                let (name, title) = entry.text.split_once('\t').unwrap_or((entry.text.as_str(), ""));
                let text = if title.is_empty() { name.to_string() } else { cut(title, MAX_SUMMARY) };
                let state = match entry.state.as_deref() {
                    Some("running") => Some("running"),
                    Some("done") => Some("done"),
                    Some("failed") => Some("failed"),
                    _ => None,
                };
                if let Some(i) = push_tool(&mut out, id, &text, Some(name.to_string()), None, state) {
                    out[i].background = entry.background;
                }
            }
            _ => {}
        }
    }
    out
}

/// The last `limit` messages of a pane's session. `source` is its usage `Context`'s: a
/// transcript or log path for Claude and Codex, a session id for OpenCode; for winfer code
/// the pane's session id.
pub fn read(provider: &str, source: &str, limit: usize) -> Option<Vec<Message>> {
    let log = || source.ends_with(".jsonl").then(|| std::fs::read_to_string(source).ok()).flatten();
    let mut messages = match provider {
        "claude" => claude(&log()?),
        "codex" => codex(&log()?),
        "opencode" => opencode(&crate::usage::opencode_db()?, source)?,
        "winfer" => winfer(&crate::harness::session::load(source)?),
        _ => return None,
    };
    if messages.len() > limit {
        messages.drain(..messages.len() - limit);
    }
    Some(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_a_thread_whose_agent_is_reasoning() {
        let thought = r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
        let replied = r#"{"type":"assistant","uuid":"a2","message":{"content":[{"type":"text","text":"Done."}]}}"#;
        assert_eq!(claude(thought).last().map(|m| m.role), Some("thinking"));
        assert!(claude(&format!("{thought}\n{replied}")).iter().all(|m| m.role != "thinking"));
        let reasoning = r#"{"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#;
        assert_eq!(codex(reasoning).last().map(|m| m.role), Some("thinking"));
    }

    #[test]
    fn reads_a_claude_transcript_as_chat() {
        let transcript = [
            r#"{"type":"user","uuid":"u0","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>meta</local-command-caveat>"}}"#,
            r#"{"type":"user","uuid":"u1","timestamp":"2026-10-19T17:09:57.903Z","message":{"role":"user","content":"fix the login bug"}}"#,
            r#"{"type":"user","uuid":"u2","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>ignore</system-reminder>"}]}}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Looking."}]}}"#,
            r#"{"type":"assistant","uuid":"a2","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test\nmore"}}]}}"#,
            r#"{"type":"user","uuid":"u3","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"assistant","uuid":"a3","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}"#,
            r#"{"type":"assistant","uuid":"a4","message":{"content":[{"type":"tool_use","id":"t2","name":"mcp__winfer__notify_user","input":{}}]}}"#,
            r#"{"type":"assistant","uuid":"a5","message":{"content":[{"type":"text","text":"Fixed it."}]}}"#,
            r#"{"type":"assistant","uuid":"a6","message":{"content":[{"type":"text","text":"Tests pass."}]}}"#,
        ]
        .join("\n");
        let m = claude(&transcript);
        let shape: Vec<(&str, &str)> = m.iter().map(|m| (m.role, m.text.as_str())).collect();
        assert_eq!(
            shape,
            [("user", "fix the login bug"), ("assistant", "Looking."), ("tool", "Bash: npm test"), ("tool", "notify_user"), ("assistant", "Fixed it.\n\nTests pass.")]
        );
        assert_eq!(m[0].ts, Some(1_792_429_797_903));
        assert_eq!(m[2].tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn reads_a_codex_log_as_chat() {
        let log = [
            r#"{"timestamp":"2026-10-19T12:32:21.747Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"m1","role":"user","content":[{"type":"input_text","text":"add a test"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"add a test"}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}","call_id":"c1"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"ok"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"m2","role":"assistant","content":[{"type":"output_text","text":"Done."}]}}"#,
        ]
        .join("\n");
        let m = codex(&log);
        let shape: Vec<(&str, &str)> = m.iter().map(|m| (m.role, m.text.as_str())).collect();
        assert_eq!(shape, [("user", "add a test"), ("tool", "exec_command: cargo test"), ("assistant", "Done.")]);
    }

    #[test]
    fn parses_timestamps_and_cuts_long_text() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ts("2000-03-01T00:00:01.5Z"), Some(951_868_801_500));
        assert_eq!(parse_ts("garbage"), None);
        assert_eq!(cut("héllo", 2), "h…");
    }

    #[test]
    fn reads_a_winfer_code_session_as_chat() {
        use crate::harness::session::{Entry, Saved};
        let e = |role: &str, text: &str| Entry { role: role.into(), text: text.into(), ..Default::default() };
        let saved = Saved {
            transcript: vec![
                e("user", "fix the build"),
                e("assistant", "Looking."),
                e("tool", "Bash\tpnpm build"),
                e("notice", "Context at 80%; compacting…"),
                e("assistant", "Fixed."),
                e("tool", "Read"),
            ],
            ..Default::default()
        };
        let m = winfer(&saved);
        let view: Vec<_> = m.iter().map(|m| (m.role, m.text.as_str(), m.tool.as_deref())).collect();
        assert_eq!(
            view,
            [
                ("user", "fix the build", None),
                ("assistant", "Looking.", None),
                ("tool", "pnpm build", Some("Bash")),
                ("assistant", "Fixed.", None),
                ("tool", "Read", Some("Read")),
            ]
        );
    }

    #[test]
    fn tracks_claude_tool_calls_and_background_work_until_they_report_back() {
        let line = |v: serde_json::Value| v.to_string();
        let use_ = |id: &str, name: &str, input: serde_json::Value| line(serde_json::json!({"type": "assistant", "uuid": format!("a-{id}"), "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}));
        let result = |id: &str, text: &str, err: bool| line(serde_json::json!({"type": "user", "uuid": format!("r-{id}"), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": err}]}}));
        let transcript = [
            use_("t1", "Read", serde_json::json!({"file_path": "src/a.ts"})),
            result("t1", "file contents", false),
            use_("t2", "Bash", serde_json::json!({"command": "pnpm dev", "run_in_background": true})),
            result("t2", "Command running in background with ID: b7x. Output is being written to: /tmp/b7x.output", false),
            use_("t3", "Agent", serde_json::json!({"description": "Explore the API", "prompt": "…"})),
            result("t3", "Async agent launched successfully.\nagentId: a9f (internal ID)", false),
            use_("t4", "Bash", serde_json::json!({"command": "false"})),
            result("t4", "exit 1", true),
            line(serde_json::json!({"type": "user", "uuid": "n1", "message": {"content": "<task-notification>\n<task-id>a9f</task-id>\n<status>completed</status>\n<summary>Agent done</summary>\n</task-notification>"}})),
            use_("t5", "Grep", serde_json::json!({"pattern": "TODO"})),
        ]
        .join("\n");
        let m = claude(&transcript);
        let view: Vec<_> = m.iter().map(|m| (m.tool.as_deref().unwrap_or(""), m.state, m.background)).collect();
        assert_eq!(view.len(), 5, "the notification isn't a user message: {m:?}");
        assert_eq!(view[0].1, Some("done"));
        assert_eq!((view[1].1, view[1].2), (Some("running"), true), "background shell still running");
        assert_eq!((view[2].1, view[2].2), (Some("done"), true), "background agent reported back");
        assert_eq!(view[3].1, Some("failed"));
        assert_eq!(view[4].1, Some("running"), "no result yet");
    }

    #[test]
    fn keeps_winfer_code_tool_states() {
        use crate::harness::session::{Entry, Saved};
        let saved = Saved {
            transcript: vec![
                Entry { role: "tool".into(), text: "Bash\tpnpm dev".into(), state: Some("running".into()), background: true },
                Entry { role: "tool".into(), text: "Read\tsrc/a.ts".into(), state: Some("done".into()), background: false },
            ],
            ..Default::default()
        };
        let m = winfer(&saved);
        assert_eq!((m[0].state, m[0].background), (Some("running"), true));
        assert_eq!((m[1].state, m[1].background), (Some("done"), false));
    }
}
