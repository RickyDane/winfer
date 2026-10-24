//! What an agent's context window is made of: system prompt, tools, instructions, the
//! conversation and tool output. Agents report only the total, so the parts are estimated from
//! the session's conversation (Claude's transcript, Codex's session log, OpenCode's database):
//! measured in characters, then scaled to add up to the reported total. What the agent sends
//! before the conversation (system prompt and tools) comes from the first request's token count
//! when the transcript doesn't say.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub kind: &'static str,
    pub label: &'static str,
    pub tokens: u64,
    /// What it's made of, biggest first: for tool results, the tools that returned them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Source {
    pub label: String,
    pub tokens: u64,
}

/// Tool results get this many sources listed by name; the rest go under "Other".
const TOP_SOURCES: usize = 4;

/// A tool's name as the breakdown shows it: an MCP tool without its `mcp__server__` prefix.
pub(crate) fn tool_label(name: &str) -> String {
    match name.strip_prefix("mcp__") {
        Some(rest) => rest.split_once("__").map_or(rest, |(_, tool)| tool).to_string(),
        None => name.to_string(),
    }
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
    /// Tool-result characters per tool.
    results: HashMap<String, u64>,
    /// Tool names by call id, to name the results that answer them.
    calls: HashMap<String, String>,
}

impl Tally {
    fn add(&mut self, kind: usize, chars: u64) {
        self.chars[kind] += chars;
    }

    /// A tool result: answering call `id` (named by [`Tally::call`]), or from tool `name`.
    fn result(&mut self, id: Option<&str>, name: Option<&str>, chars: u64) {
        self.add(TOOL_RESULTS, chars);
        let label = name.map(tool_label).or_else(|| id.and_then(|id| self.calls.get(id).cloned())).unwrap_or_else(|| "Other".into());
        *self.results.entry(label).or_default() += chars;
    }

    /// A tool call, remembered to name its result.
    fn call(&mut self, id: Option<&str>, name: Option<&str>) {
        if let (Some(id), Some(name)) = (id, name) {
            self.calls.insert(id.to_string(), tool_label(name));
        }
    }

    /// Tool results per tool, scaled like the rest, biggest first.
    fn sources(&self, scale: f64) -> Vec<Source> {
        let mut by_tool: Vec<(&String, u64)> = self.results.iter().map(|(k, v)| (k, *v)).collect();
        by_tool.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let mut sources: Vec<Source> = Vec::new();
        let mut other = 0;
        for (label, chars) in by_tool {
            if sources.len() < TOP_SOURCES && label != "Other" {
                sources.push(Source { label: label.clone(), tokens: (chars as f64 * scale).round() as u64 });
            } else {
                other += chars;
            }
        }
        sources.push(Source { label: "Other".into(), tokens: (other as f64 * scale).round() as u64 });
        sources.retain(|s| s.tokens > 0);
        sources
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
                    parts.push(Part { kind: "setup", label: "System prompt & tools", tokens: fixed, sources: Vec::new() });
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
                let sources = if i == TOOL_RESULTS { self.sources(scale) } else { Vec::new() };
                parts.push(Part { kind, label, tokens, sources });
            }
        }
        parts
    }
}

/// Characters of text in a value: its strings, not ids, signatures or encrypted blobs; an image
/// counts as what it costs.
pub(crate) fn text_len(v: &Value) -> u64 {
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
pub(crate) fn claude_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<system-reminder>") || t.starts_with("<local-command-") || t.starts_with("<command-")
}

/// From a Claude Code transcript (JSONL), since its last compaction.
pub fn claude_breakdown(transcript: &str, total: u64) -> Vec<Part> {
    let mut t = Tally::default();
    // The latest request's system prompt and tools, which outlive compactions.
    let mut shape: Option<Value> = None;
    // Deferred tools (`defer_loading`) are in the context only once tool search has loaded them
    // (a `tool_reference` names them); the others are placeholders the model doesn't see.
    let mut loaded: std::collections::HashSet<String> = Default::default();
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match v["type"].as_str().unwrap_or_default() {
            "api-request-shape" => shape = Some(v["shape"].clone()),
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
                                "tool_result" => {
                                    for r in b["content"].as_array().into_iter().flatten().filter(|r| r["type"] == "tool_reference") {
                                        loaded.extend(r["tool_name"].as_str().map(String::from));
                                    }
                                    t.result(b["tool_use_id"].as_str(), None, text_len(&b["content"]));
                                }
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
                        "tool_use" | "server_tool_use" => {
                            t.call(b["id"].as_str(), b["name"].as_str());
                            t.add(TOOL_CALLS, text_len(&b["name"]) + b["input"].to_string().len() as u64);
                        }
                        "web_search_tool_result" => t.result(b["tool_use_id"].as_str(), None, text_len(&b["content"])),
                        _ => t.add(REPLIES, text_len(b)),
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(shape) = shape {
        let (mut tools, mut mcp) = (0, 0);
        for tool in shape["tools"].as_array().into_iter().flatten() {
            let name = tool["name"].as_str().unwrap_or_default();
            if tool["defer_loading"].as_bool() == Some(true) && !loaded.contains(name) {
                continue;
            }
            let len = tool.to_string().len() as u64;
            if name.starts_with("mcp__") { mcp += len } else { tools += len }
        }
        t.chars[SYSTEM] = text_len(&shape["system"]);
        t.chars[TOOLS] = tools;
        t.chars[MCP] = mcp;
    }
    t.parts(total)
}

// Codex ------------------------------------------------------------------------------------------

/// Context Codex adds as user messages (environment, AGENTS.md, permissions).
pub(crate) fn codex_injected(text: &str) -> bool {
    let t = text.trim_start();
    ["<environment_context>", "<user_instructions>", "# AGENTS.md", "<permissions", "<skills_instructions>", "<turn_aborted>"]
        .iter()
        .any(|p| t.starts_with(p))
}

fn codex_item(t: &mut Tally, item: &Value) {
    // Input messages may leave out their type (winfer code's ChatGPT backend does).
    let kind = item["type"].as_str().unwrap_or(if item["role"].is_string() { "message" } else { "" });
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
        k if k.ends_with("_output") => t.result(item["call_id"].as_str(), None, text_len(item)),
        k if k.ends_with("_call") => {
            t.output_starts();
            // Function and custom tools are named; built-in ones (`local_shell_call`) by their type.
            let name = item["name"].as_str().unwrap_or_else(|| k.trim_end_matches("_call"));
            t.call(item["call_id"].as_str(), Some(name));
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

// winfer code's ChatGPT backend ------------------------------------------------------------------

/// From a ChatGPT-backend session (`winfer code`): its saved items, which are Responses API items
/// like Codex's, and the size of the instructions and tools sent with them when known.
pub fn chatgpt_breakdown(items: &str, setup: Option<&str>, total: u64) -> Option<Vec<Part>> {
    let items: Vec<Value> = serde_json::from_str(items).ok()?;
    let mut t = Tally::default();
    items.iter().for_each(|item| codex_item(&mut t, item));
    if let Some(setup) = setup.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        let n = |k: &str| setup[k].as_u64().unwrap_or(0);
        t.chars[SYSTEM] = n("system");
        t.chars[TOOLS] = n("tools");
        t.chars[MCP] = n("mcp");
    }
    Some(t.parts(total))
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
                            t.result(None, part["name"].as_str(), text_len(&state["content"]) + text_len(&state["error"]));
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
    tauri::async_runtime::spawn_blocking(move || breakdown(&provider, &source, tokens)).await.ok().flatten()
}

/// `usage_context_breakdown`'s work, also run by `winfer code --serve` for its `/cost` sheet.
pub fn breakdown(provider: &str, source: &str, tokens: u64) -> Option<Vec<Part>> {
    let log = || (source.ends_with(".jsonl")).then(|| std::fs::read_to_string(source).ok()).flatten();
    match provider {
        "claude" => Some(claude_breakdown(&log()?, tokens)),
        "codex" => Some(codex_breakdown(&log()?, tokens)),
        "opencode" => opencode_breakdown(&crate::usage::opencode_db()?, source, tokens),
        "chatgpt" => {
            let path = std::path::Path::new(source);
            let setup = std::fs::read_to_string(path.with_extension("setup.json")).ok();
            chatgpt_breakdown(&std::fs::read_to_string(path).ok()?, setup.as_deref(), tokens)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(parts: &[Part], kind: &str) -> u64 {
        parts.iter().find(|p| p.kind == kind).map_or(0, |p| p.tokens)
    }

    /// The tools named under tool results.
    fn sources(parts: &[Part]) -> Vec<&str> {
        parts.iter().find(|p| p.kind == "toolResults").map_or(Vec::new(), |p| p.sources.iter().map(|s| s.label.as_str()).collect())
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
        assert_eq!(sources(&parts), ["Read"]);
        // The compacted question and the subagent aren't counted: messages is the one sentence.
        assert!(get(&parts, "messages") < get(&parts, "instructions") * 2);
    }

    #[test]
    fn counts_deferred_tools_once_loaded() {
        let schema = "x".repeat(400);
        let shape = |tools: &str| format!(r#"{{"type":"api-request-shape","shape":{{"system":[{{"type":"text","text":"s"}}],"tools":[{{"name":"Read","description":"d"}},{tools}]}}}}"#);
        let deferred = |name: &str| format!(r#"{{"name":"{name}","description":"{schema}","defer_loading":true}}"#);
        let run = |lines: &[String]| {
            let parts = claude_breakdown(&lines.join("\n"), 10_000);
            (get(&parts, "mcp"), get(&parts, "tools"))
        };
        // Not loaded: a placeholder and an MCP tool tool search hasn't returned yet count nothing.
        let before = shape(&format!("{},{}", deferred("DeferredToolPlaceholder"), deferred("mcp__winfer__list_panes")));
        let reply = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}"#.to_string();
        assert_eq!(run(&[before.clone(), reply.clone()]).0, 0);
        // Loaded by tool search: the MCP tool is in the context, the placeholder still isn't.
        let found = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"tool_reference","tool_name":"mcp__winfer__list_panes"}]}]}}"#.to_string();
        let (mcp, tools) = run(&[found, before, reply]);
        assert!(mcp > tools * 10, "{mcp} {tools}");
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
        assert_eq!(sources(&parts), ["shell"]);
    }

    #[test]
    fn names_tool_results_by_tool() {
        let mut t = Tally::default();
        for (i, (name, chars)) in [("Bash", 600), ("Read", 300), ("mcp__winfer__kb_query", 50), ("Grep", 40), ("Glob", 20), ("Edit", 10)].iter().enumerate() {
            let id = i.to_string();
            t.call(Some(&id), Some(name));
            t.result(Some(&id), None, *chars);
        }
        t.result(Some("unknown"), None, 5);
        let parts = t.parts(1025);
        let results = parts.iter().find(|p| p.kind == "toolResults").unwrap();
        let named: Vec<(&str, u64)> = results.sources.iter().map(|s| (s.label.as_str(), s.tokens)).collect();
        // Top four by size, MCP tools without their server prefix, the rest (and unnamed) as Other.
        assert_eq!(named, [("Bash", 600), ("Read", 300), ("kb_query", 50), ("Grep", 40), ("Other", 35)]);
        assert_eq!(tool_label("mcp__winfer-browser__browser_snapshot"), "browser_snapshot");
    }

    #[test]
    fn splits_a_chatgpt_session() {
        let items = r#"[{"role":"user","content":[{"type":"input_text","text":"fix it"}]},{"type":"function_call","name":"read","arguments":"{\"path\":\"a\"}","call_id":"c"},{"type":"function_call_output","call_id":"c","output":"lots of file content here"},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]"#;
        let parts = chatgpt_breakdown(items, Some(r#"{"system":400,"tools":800,"mcp":0}"#), 1000).unwrap();
        assert!(get(&parts, "tools") > get(&parts, "system") && get(&parts, "system") > get(&parts, "toolResults"), "{parts:?}");
        assert!(get(&parts, "messages") > 0 && get(&parts, "toolCalls") > 0 && get(&parts, "replies") > 0);
        assert_eq!(get(&parts, "mcp"), 0);
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
        assert_eq!(sources(&parts), ["shell"]);
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

    /// Breakdowns of this machine's newest Codex log and OpenCode session:
    /// `cargo test breakdowns_on_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn breakdowns_on_this_machine() {
        let cwd = env!("CARGO_MANIFEST_DIR").trim_end_matches("/src-tauri");
        if let Some(log) = crate::usage::codex_log(None, cwd) {
            println!("codex {log:?}\n{:#?}", codex_breakdown(&std::fs::read_to_string(&log).unwrap(), 100_000));
        }
        if let Some(conn) = crate::usage::opencode_db() {
            let session: Option<String> = conn.query_row("select session_id from session_message where type = 'assistant' and json_extract(data, '$.tokens.input') > 0 order by rowid desc limit 1", [], |r| r.get(0)).ok();
            if let Some(session) = session {
                println!("opencode {session}\n{:#?}", opencode_breakdown(&conn, &session, 100_000));
            }
        }
        let chatgpt = crate::harness::config::dir().join("chatgpt");
        let newest = std::fs::read_dir(chatgpt).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| !p.to_string_lossy().ends_with(".setup.json")).max_by_key(|p| p.metadata().unwrap().modified().unwrap());
        if let Some(items) = newest {
            let setup = std::fs::read_to_string(items.with_extension("setup.json")).ok();
            println!("chatgpt {items:?}\n{:#?}", chatgpt_breakdown(&std::fs::read_to_string(&items).unwrap(), setup.as_deref(), 100_000));
        }
    }
}
