//! What an agent's context window is made of: system prompt, tools, instructions, the
//! conversation and tool output. Agents report only the total, so the parts are estimated from
//! the session's conversation (Claude's transcript, Codex's session log, OpenCode's database):
//! measured in characters, then scaled to add up to the reported total. What the agent sends
//! before the conversation (system prompt and tools) comes from the first request's token count
//! when the transcript doesn't say.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub kind: &'static str,
    pub label: &'static str,
    pub tokens: u64,
}

const SYSTEM: usize = 0;
const TOOLS: usize = 1;
const MCP: usize = 2;
const INSTRUCTIONS: usize = 3;
const MESSAGES: usize = 4;
const REPLIES: usize = 5;
const THINKING: usize = 6;
const TOOL_CALLS: usize = 7;
const TOOL_RESULTS: usize = 8;

const KINDS: [(&str, &str); 9] = [
    ("system", "System prompt"),
    ("tools", "Tools"),
    ("mcp", "MCP tools"),
    ("instructions", "Memory & instructions"),
    ("messages", "Your messages"),
    ("replies", "Replies"),
    ("thinking", "Thinking"),
    ("toolCalls", "Tool calls"),
    ("toolResults", "Tool results"),
];

/// An image costs about this many tokens whatever its size in the log.
const IMAGE_CHARS: u64 = 1600 * 4;
const CHARS_PER_TOKEN: f64 = 4.0;

#[derive(Default)]
struct Tally {
    /// Characters per kind.
    chars: [u64; 9],
    /// The first request's prompt tokens and the conversation's characters sent with it.
    first_prompt: Option<u64>,
    sent_with_first: Option<u64>,
}

impl Tally {
    fn add(&mut self, kind: usize, chars: u64) {
        self.chars[kind] += chars;
    }

    fn conversation(&self) -> u64 {
        self.chars[INSTRUCTIONS..].iter().sum()
    }

    /// The first output of the session: what was sent before it went with the first request.
    fn output_starts(&mut self) {
        if self.sent_with_first.is_none() {
            self.sent_with_first = Some(self.conversation());
        }
    }

    /// The parts, adding up to `total` tokens.
    fn parts(&self, total: u64) -> Vec<Part> {
        let mut parts = Vec::new();
        // Measured, so kept as is; only when the transcript has no system prompt or tools of its own.
        let mut fixed = 0;
        if self.chars[SYSTEM..=MCP].iter().all(|c| *c == 0) {
            if let Some(prompt) = self.first_prompt {
                let sent = self.sent_with_first.unwrap_or_else(|| self.conversation());
                fixed = prompt.saturating_sub((sent as f64 / CHARS_PER_TOKEN) as u64).min(total);
                if fixed > 0 {
                    parts.push(Part { kind: "setup", label: "System prompt & tools", tokens: fixed });
                }
            }
        }
        let estimated: u64 = self.chars.iter().sum();
        if estimated == 0 {
            return parts;
        }
        let scale = (total - fixed) as f64 / estimated as f64;
        for (i, (kind, label)) in KINDS.iter().enumerate() {
            let tokens = (self.chars[i] as f64 * scale).round() as u64;
            if tokens > 0 {
                parts.push(Part { kind, label, tokens });
            }
        }
        parts
    }
}

/// Characters of text in a value: its strings, not ids, signatures or encrypted blobs; an image
/// counts as what it costs.
fn text_len(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.len() as u64,
        Value::Array(a) => a.iter().map(text_len).sum(),
        Value::Object(o) => {
            if matches!(o.get("type").and_then(Value::as_str), Some("image" | "input_image" | "document")) {
                return IMAGE_CHARS;
            }
            o.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "id" | "type" | "call_id" | "tool_use_id" | "status" | "signature" | "encrypted_content" | "uuid"))
                .map(|(_, v)| text_len(v))
                .sum()
        }
        _ => 0,
    }
}

// Claude Code ------------------------------------------------------------------------------------

/// Injected context (CLAUDE.md, hook output, reminders) rather than something the user typed.
fn claude_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<system-reminder>") || t.starts_with("<local-command-") || t.starts_with("<command-")
}

/// From a Claude Code transcript (JSONL), since its last compaction.
pub fn claude_breakdown(transcript: &str, total: u64) -> Vec<Part> {
    let mut t = Tally::default();
    // The latest request's system prompt and tools, which outlive compactions.
    let mut setup: Option<[u64; 3]> = None;
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match v["type"].as_str().unwrap_or_default() {
            "api-request-shape" => {
                let shape = &v["shape"];
                let (mut tools, mut mcp) = (0, 0);
                for tool in shape["tools"].as_array().into_iter().flatten() {
                    let len = tool.to_string().len() as u64;
                    if tool["name"].as_str().is_some_and(|n| n.starts_with("mcp__")) { mcp += len } else { tools += len }
                }
                setup = Some([text_len(&shape["system"]), tools, mcp]);
            }
            "system" if v["subtype"].as_str() == Some("compact_boundary") => t = Tally::default(),
            "attachment" => {
                // Snapshots and records are bookkeeping, not sent to the model.
                let kind = v["attachment"]["type"].as_str().unwrap_or_default();
                if !kind.contains("snapshot") && !kind.ends_with("_record") {
                    t.add(INSTRUCTIONS, text_len(&v["attachment"]));
                }
            }
            "user" => {
                let meta = v["isMeta"].as_bool() == Some(true);
                let text_kind = |s: &str| if meta || claude_injected(s) { INSTRUCTIONS } else { MESSAGES };
                match &v["message"]["content"] {
                    Value::String(s) => t.add(text_kind(s), s.len() as u64),
                    Value::Array(blocks) => {
                        for b in blocks {
                            match b["type"].as_str().unwrap_or_default() {
                                "tool_result" => t.add(TOOL_RESULTS, text_len(&b["content"])),
                                "text" => {
                                    let s = b["text"].as_str().unwrap_or_default();
                                    t.add(text_kind(s), s.len() as u64);
                                }
                                _ => t.add(if meta { INSTRUCTIONS } else { MESSAGES }, text_len(b)),
                            }
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                let message = &v["message"];
                if t.first_prompt.is_none() {
                    let u = &message["usage"];
                    let n = |k: &str| u[k].as_u64().unwrap_or(0);
                    let prompt = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                    if prompt > 0 {
                        t.first_prompt = Some(prompt);
                    }
                }
                t.output_starts();
                for b in message["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str().unwrap_or_default() {
                        "text" => t.add(REPLIES, text_len(&b["text"])),
                        "thinking" => t.add(THINKING, text_len(&b["thinking"])),
                        "redacted_thinking" => {}
                        "tool_use" | "server_tool_use" => t.add(TOOL_CALLS, text_len(&b["name"]) + b["input"].to_string().len() as u64),
                        "web_search_tool_result" => t.add(TOOL_RESULTS, text_len(&b["content"])),
                        _ => t.add(REPLIES, text_len(b)),
                    }
                }
            }
            _ => {}
        }
    }
    if let Some([system, tools, mcp]) = setup {
        t.chars[SYSTEM] = system;
        t.chars[TOOLS] = tools;
        t.chars[MCP] = mcp;
    }
    t.parts(total)
}

// Codex ------------------------------------------------------------------------------------------

/// Context Codex adds as user messages (environment, AGENTS.md, permissions).
fn codex_injected(text: &str) -> bool {
    let t = text.trim_start();
    ["<environment_context>", "<user_instructions>", "# AGENTS.md", "<permissions", "<skills_instructions>", "<turn_aborted>"]
        .iter()
        .any(|p| t.starts_with(p))
}

fn codex_item(t: &mut Tally, item: &Value) {
    let kind = item["type"].as_str().unwrap_or_default();
    match kind {
        "message" => {
            let text = item["content"].as_array().and_then(|c| c.first()).and_then(|c| c["text"].as_str()).unwrap_or_default();
            let len = text_len(&item["content"]);
            match item["role"].as_str().unwrap_or_default() {
                "developer" | "system" => t.add(INSTRUCTIONS, len),
                "user" => t.add(if codex_injected(text) { INSTRUCTIONS } else { MESSAGES }, len),
                _ => {
                    t.output_starts();
                    t.add(REPLIES, len);
                }
            }
        }
        "reasoning" => {
            t.output_starts();
            // Earlier turns' reasoning goes back encrypted; its size is a rough measure.
            let encrypted = item["encrypted_content"].as_str().map_or(0, |e| e.len() as u64 * 3 / 4);
            t.add(THINKING, text_len(&item["summary"]) + text_len(&item["content"]) + encrypted);
        }
        k if k.ends_with("_output") => t.add(TOOL_RESULTS, text_len(item)),
        k if k.ends_with("_call") => {
            t.output_starts();
            t.add(TOOL_CALLS, text_len(item));
        }
        _ => {}
    }
}

/// From a Codex session log (JSONL), since its last compaction.
pub fn codex_breakdown(log: &str, total: u64) -> Vec<Part> {
    let mut t = Tally::default();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let payload = &v["payload"];
        match v["type"].as_str().unwrap_or_default() {
            "compacted" => {
                t = Tally::default();
                match payload["replacement_history"].as_array() {
                    Some(items) => items.iter().for_each(|item| codex_item(&mut t, item)),
                    None => t.add(MESSAGES, text_len(&payload["message"])),
                }
            }
            "response_item" => codex_item(&mut t, payload),
            "event_msg" if payload["type"].as_str() == Some("token_count") && t.first_prompt.is_none() => {
                t.first_prompt = payload["info"]["last_token_usage"]["input_tokens"].as_u64().filter(|n| *n > 0);
            }
            _ => {}
        }
    }
    t.parts(total)
}

// OpenCode ---------------------------------------------------------------------------------------

/// From an OpenCode session's messages.
pub fn opencode_breakdown(conn: &rusqlite::Connection, session: &str, total: u64) -> Option<Vec<Part>> {
    let mut stmt = conn.prepare("select type, data from session_message where session_id = ?1 order by seq").ok()?;
    let rows = stmt.query_map([session], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?;
    let mut t = Tally::default();
    for (kind, raw) in rows.flatten() {
        let Ok(data) = serde_json::from_str::<Value>(&raw) else { continue };
        match kind.as_str() {
            "user" => t.add(MESSAGES, text_len(&data["text"]) + text_len(&data["files"])),
            "system" | "synthetic" => t.add(INSTRUCTIONS, text_len(&data["text"])),
            "assistant" => {
                if t.first_prompt.is_none() {
                    let tk = &data["tokens"];
                    let n = |v: &Value| v.as_u64().unwrap_or(0);
                    let prompt = n(&tk["input"]) + n(&tk["cache"]["read"]) + n(&tk["cache"]["write"]);
                    if prompt > 0 {
                        t.first_prompt = Some(prompt);
                    }
                }
                t.output_starts();
                for part in data["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str().unwrap_or_default() {
                        "reasoning" => t.add(THINKING, text_len(&part["text"])),
                        "tool" => {
                            let state = &part["state"];
                            t.add(TOOL_CALLS, text_len(&part["name"]) + state["input"].to_string().len() as u64);
                            t.add(TOOL_RESULTS, text_len(&state["content"]) + text_len(&state["error"]));
                        }
                        _ => t.add(REPLIES, text_len(&part["text"])),
                    }
                }
            }
            _ => {}
        }
    }
    Some(t.parts(total))
}

/// The parts of a pane's context use. `source` is the `Context`'s: a transcript or session log
/// path for Claude and Codex, a session id for OpenCode.
#[tauri::command]
pub async fn usage_context_breakdown(provider: String, source: String, tokens: u64) -> Option<Vec<Part>> {
    tauri::async_runtime::spawn_blocking(move || {
        let log = || (source.ends_with(".jsonl")).then(|| std::fs::read_to_string(&source).ok()).flatten();
        match provider.as_str() {
            "claude" => Some(claude_breakdown(&log()?, tokens)),
            "codex" => Some(codex_breakdown(&log()?, tokens)),
            "opencode" => opencode_breakdown(&crate::usage::opencode_db()?, &source, tokens),
            _ => None,
        }
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(parts: &[Part], kind: &str) -> u64 {
        parts.iter().find(|p| p.kind == kind).map_or(0, |p| p.tokens)
    }

    #[test]
    fn splits_a_claude_transcript() {
        let transcript = [
            r#"{"type":"user","message":{"role":"user","content":"old question before compaction"}}"#,
            r#"{"type":"system","subtype":"compact_boundary"}"#,
            r#"{"type":"api-request-shape","shape":{"system":[{"type":"text","text":"SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSS"}],"tools":[{"name":"Read","description":"d"},{"name":"mcp__x__y","description":"d"}]}}"#,
            r#"{"type":"attachment","attachment":{"type":"hook_success","content":"MMMMMMMMMMMMMMMMMMMM"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"hello there, fix the bug"}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent prompt that is not in this context at all"}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":3,"cache_read_input_tokens":100},"content":[{"type":"thinking","thinking":"hmm","signature":"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_use_id","tool_use_id":"t1"},{"type":"tool_result","tool_use_id":"t1","content":"RRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRR"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#,
        ]
        .join("\n");
        let parts = claude_breakdown(&transcript, 1000);
        let sum: u64 = parts.iter().map(|p| p.tokens).sum();
        assert!((995..=1005).contains(&sum), "{parts:?}");
        assert!(get(&parts, "toolResults") > get(&parts, "messages"));
        assert!(get(&parts, "system") > get(&parts, "instructions"));
        assert!(get(&parts, "mcp") > 0 && get(&parts, "tools") > 0 && get(&parts, "replies") > 0);
        assert_eq!(get(&parts, "setup"), 0);
        // The compacted question and the subagent aren't counted: messages is the one sentence.
        assert!(get(&parts, "messages") < get(&parts, "instructions") * 2);
    }

    #[test]
    fn measures_the_setup_without_a_request_shape() {
        let transcript = [
            r#"{"type":"user","message":{"role":"user","content":"12345678"}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":502},"content":[{"type":"text","text":"12345678"}]}}"#,
        ]
        .join("\n");
        let parts = claude_breakdown(&transcript, 504);
        // 502 prompt tokens, 2 of them the message.
        assert_eq!(get(&parts, "setup"), 500);
        assert_eq!((get(&parts, "messages"), get(&parts, "replies")), (2, 2));
    }

    #[test]
    fn splits_a_codex_log() {
        let log = [
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"DDDDDDDDDDDDDDDD"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"do it"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"ls\"}","call_id":"c1"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"a.txt b.txt c.txt d.txt e.txt f.txt"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000}}}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#,
        ]
        .join("\n");
        let parts = codex_breakdown(&log, 2000);
        assert!(get(&parts, "setup") > 950, "{parts:?}");
        assert!(get(&parts, "instructions") > get(&parts, "messages"));
        assert!(get(&parts, "toolResults") > 0 && get(&parts, "toolCalls") > 0 && get(&parts, "replies") > 0);
    }

    #[test]
    fn splits_an_opencode_session() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"create table session_message (session_id text, type text, seq integer, data text);
            insert into session_message values ('s1', 'user', 1, '{"text":"run the tests please"}');
            insert into session_message values ('s1', 'assistant', 2, '{"tokens":{"input":800,"cache":{"read":200,"write":0}},"content":[{"type":"reasoning","text":"think"},{"type":"tool","name":"shell","state":{"input":{"command":"npm test"},"content":[{"type":"text","text":"all 42 tests passed in 3.1 seconds"}]}}]}');
            insert into session_message values ('s2', 'user', 1, '{"text":"other session"}');"#,
        )
        .unwrap();
        let parts = opencode_breakdown(&conn, "s1", 1100).unwrap();
        assert!(get(&parts, "setup") > 990, "{parts:?}");
        assert!(get(&parts, "toolResults") > 0 && get(&parts, "messages") > 0 && get(&parts, "thinking") > 0);
    }

    /// This machine's newest Claude transcript: `cargo test claude_breakdown_on_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn claude_breakdown_on_this_machine() {
        let dir = dirs::home_dir().unwrap().join(".claude/projects/-Users-rickyperlick-Coding-winfer");
        let newest = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .max_by_key(|p| p.metadata().unwrap().modified().unwrap())
            .unwrap();
        println!("{newest:?}\n{:#?}", claude_breakdown(&std::fs::read_to_string(&newest).unwrap(), 100_000));
    }
}
