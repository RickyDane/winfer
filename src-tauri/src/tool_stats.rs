//! What the agents' tool calls and shell commands cost: how often each ran, how many failed and
//! which, how long they took and roughly how many tokens they put through the context. Read from
//! the agents' own logs, like the usage history: Claude Code's transcripts and Codex's session logs.
//!
//! A call counts once its result is in the log. Its time runs from the call to the result; its
//! tokens are estimated from the characters of the call's input and its result.

use crate::context_breakdown::{text_len, tool_label};
use crate::usage_history::{claude_projects, codex_sessions, jsonl_files};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

const CHARS_PER_TOKEN: u64 = 4;
/// Failed calls listed, newest first.
const MAX_FAILURES: usize = 100;
const INPUT_CHARS: usize = 200;
const ERROR_CHARS: usize = 400;

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub name: String,
    pub calls: u64,
    pub failed: u64,
    /// Estimated: the call's input and its result.
    pub tokens: u64,
    /// Summed over the calls whose time is known (`timed` of them).
    pub duration_ms: u64,
    pub max_ms: u64,
    pub timed: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub tool: String,
    pub provider: &'static str,
    /// Unix ms.
    pub at: i64,
    /// What it was asked to do (command, file, pattern…), shortened.
    pub input: String,
    pub error: String,
    pub cwd: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub calls: u64,
    pub failed: u64,
    pub tokens: u64,
    pub duration_ms: u64,
    /// Sessions with at least one tool call.
    pub sessions: u64,
    /// Most called first.
    pub tools: Vec<Stat>,
    /// Shell commands by program (`git`, `npm`, …), most run first.
    pub commands: Vec<Stat>,
    pub failures: Vec<Failure>,
}

/// A call waiting for its result.
struct Call {
    tool: String,
    at: Option<i64>,
    chars: u64,
    input: String,
    command: Option<String>,
}

/// How a call ended.
struct Outcome {
    at: Option<i64>,
    failed: bool,
    /// When the log says; otherwise the time from call to result.
    ms: Option<u64>,
    chars: u64,
    error: String,
}

#[derive(Default)]
struct Acc {
    tools: HashMap<String, Stat>,
    commands: HashMap<String, Stat>,
    failures: Vec<Failure>,
    sessions: u64,
}

fn bump(stat: &mut Stat, failed: bool, tokens: u64, ms: Option<u64>) {
    stat.calls += 1;
    stat.failed += failed as u64;
    stat.tokens += tokens;
    if let Some(ms) = ms {
        stat.duration_ms += ms;
        stat.max_ms = stat.max_ms.max(ms);
        stat.timed += 1;
    }
}

impl Acc {
    fn record(&mut self, provider: &'static str, call: Call, out: Outcome, cwd: Option<&str>) {
        let ms = out.ms.or_else(|| Some(out.at? - call.at?).filter(|d| *d >= 0).map(|d| d as u64));
        let tokens = (call.chars + out.chars).div_ceil(CHARS_PER_TOKEN);
        bump(self.tools.entry(call.tool.clone()).or_default(), out.failed, tokens, ms);
        if let Some(cmd) = &call.command {
            bump(self.commands.entry(program(cmd)).or_default(), out.failed, tokens, ms);
        }
        if out.failed {
            self.failures.push(Failure {
                tool: call.tool,
                provider,
                at: out.at.or(call.at).unwrap_or(0),
                input: call.input,
                error: shorten(out.error.trim(), ERROR_CHARS),
                cwd: cwd.map(String::from),
                duration_ms: ms,
            });
        }
    }

    /// A shell command the log reports on its own (Codex), counted under commands only.
    fn command(&mut self, cmd: &str, failed: bool, ms: Option<u64>, chars: u64) {
        bump(self.commands.entry(program(cmd)).or_default(), failed, chars.div_ceil(CHARS_PER_TOKEN), ms);
    }

    fn report(self) -> Report {
        let sorted = |m: HashMap<String, Stat>| {
            let mut v: Vec<Stat> = m.into_iter().map(|(name, s)| Stat { name, ..s }).collect();
            v.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
            v
        };
        let tools = sorted(self.tools);
        let mut failures = self.failures;
        failures.sort_by_key(|f| std::cmp::Reverse(f.at));
        failures.truncate(MAX_FAILURES);
        Report {
            calls: tools.iter().map(|s| s.calls).sum(),
            failed: tools.iter().map(|s| s.failed).sum(),
            tokens: tools.iter().map(|s| s.tokens).sum(),
            duration_ms: tools.iter().map(|s| s.duration_ms).sum(),
            sessions: self.sessions,
            tools,
            commands: sorted(self.commands),
            failures,
        }
    }
}

fn shorten(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

fn timestamp(v: &Value) -> Option<i64> {
    v["timestamp"].as_str().and_then(crate::usage::parse_rfc3339)
}

/// The text of a result: a string, or the text blocks of a list.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|b| b["text"].as_str().map_or_else(|| text_of(b), String::from)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The program a shell command runs: its first word after `cd …&&`, env assignments and wrappers.
pub(crate) fn program(cmd: &str) -> String {
    let mut saw_cd = false;
    for seg in cmd.split(['\n', ';']).flat_map(|s| s.split("&&")).flat_map(|s| s.split("||")) {
        let word = seg
            .split_whitespace()
            .map(|w| w.trim_start_matches(['(', '{']))
            .find(|w| {
                let assignment = w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
                !w.is_empty() && !w.starts_with('-') && !assignment && !matches!(*w, "sudo" | "time" | "env" | "nohup" | "exec" | "command" | "then" | "do")
            });
        match word {
            Some("cd" | "pushd" | "popd") => saw_cd = true,
            Some(w) => return w.rsplit('/').next().unwrap_or(w).to_string(),
            None => {}
        }
    }
    if saw_cd { "cd".into() } else { "other".into() }
}

/// A shell command from a list (`["/bin/zsh", "-lc", "git status"]`) or a string.
fn command_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => {
            let parts: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            match parts.as_slice() {
                [sh, flag, script, ..] if sh.ends_with("sh") && flag.starts_with('-') && flag.contains('c') => Some(script.to_string()),
                [] => None,
                _ => Some(parts.join(" ")),
            }
        }
        _ => None,
    }
}

/// What a call was asked to do, for the failure list.
fn input_summary(input: &Value) -> String {
    let s = ["command", "cmd", "file_path", "path", "pattern", "url", "query", "description", "prompt", "skill"]
        .iter()
        .find_map(|k| match &input[*k] {
            Value::Null => None,
            v => command_of(v),
        })
        .unwrap_or_else(|| match input {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        });
    shorten(s.trim(), INPUT_CHARS)
}

/// The exit code a command's output reports, if any.
fn exit_code(text: &str) -> Option<i64> {
    ["\"exit_code\":", "Exit code: ", "Exit code ", "Process exited with code "].iter().find_map(|pat| {
        let rest = text[text.find(pat)? + pat.len()..].trim_start();
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '-')).unwrap_or(rest.len());
        rest[..end].parse().ok()
    })
}

/// A duration as Codex writes it: `{secs, nanos}` or seconds.
fn codex_ms(v: &Value) -> Option<u64> {
    match v {
        Value::Object(o) => Some(o.get("secs")?.as_u64()? * 1000 + o.get("nanos").and_then(Value::as_u64).unwrap_or(0) / 1_000_000),
        Value::Number(n) => n.as_f64().map(|s| (s * 1000.0).round() as u64),
        _ => None,
    }
}

// Claude Code ------------------------------------------------------------------------------------

fn claude_file(text: &str, since: i64, acc: &mut Acc) {
    let mut pending: HashMap<String, Call> = HashMap::new();
    let mut cwd: Option<String> = None;
    let mut any = false;
    for line in text.lines() {
        if !line.contains("\"tool_use\"") && !line.contains("\"tool_result\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = timestamp(&v);
        if let Some(c) = v["cwd"].as_str() {
            cwd = Some(c.to_string());
        }
        let blocks = v["message"]["content"].as_array().into_iter().flatten();
        match v["type"].as_str() {
            Some("assistant") => {
                for b in blocks.filter(|b| matches!(b["type"].as_str(), Some("tool_use" | "server_tool_use"))) {
                    let (Some(id), Some(name)) = (b["id"].as_str(), b["name"].as_str()) else { continue };
                    let input = &b["input"];
                    pending.insert(
                        id.to_string(),
                        Call {
                            tool: tool_label(name),
                            at,
                            chars: name.len() as u64 + input.to_string().len() as u64,
                            input: input_summary(input),
                            command: (name == "Bash").then(|| input["command"].as_str().map(String::from)).flatten(),
                        },
                    );
                }
            }
            Some("user") => {
                for b in blocks.filter(|b| b["type"] == "tool_result") {
                    let Some(call) = b["tool_use_id"].as_str().and_then(|id| pending.remove(id)) else { continue };
                    if at.or(call.at).is_none_or(|t| t < since) {
                        continue;
                    }
                    let failed = b["is_error"].as_bool() == Some(true);
                    let out = Outcome {
                        at,
                        failed,
                        ms: None,
                        chars: text_len(&b["content"]),
                        error: if failed { text_of(&b["content"]) } else { String::new() },
                    };
                    acc.record("claude", call, out, cwd.as_deref());
                    any = true;
                }
            }
            _ => {}
        }
    }
    acc.sessions += any as u64;
}

// Codex ------------------------------------------------------------------------------------------

/// The shell command of a call to Codex's shell tools.
fn codex_call_command(name: &str, args: &Value) -> Option<String> {
    if !matches!(name, "shell" | "local_shell" | "exec_command" | "container.exec" | "shell_command") {
        return None;
    }
    ["command", "cmd"].iter().find_map(|k| command_of(&args[*k])).or_else(|| command_of(&args["action"]["command"]))
}

fn codex_file(text: &str, since: i64, acc: &mut Acc) {
    let mut pending: HashMap<String, Call> = HashMap::new();
    let mut cwd: Option<String> = None;
    let mut any = false;
    for line in text.lines() {
        if !["_call", "_output", "CommandExecution", "exec_command_end", "turn_context", "session_meta"].iter().any(|k| line.contains(k)) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = timestamp(&v);
        let p = &v["payload"];
        if let Some(c) = p["cwd"].as_str() {
            cwd = Some(c.to_string());
        }
        let kind = p["type"].as_str().unwrap_or_default();
        match v["type"].as_str() {
            Some("response_item") if kind.ends_with("_call") => {
                let Some(id) = p["call_id"].as_str() else { continue };
                let name = p["name"].as_str().unwrap_or_else(|| kind.trim_end_matches("_call"));
                // Function arguments come as a JSON string; custom tools take free text.
                let args = match &p["arguments"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
                    Value::Null => p.get("input").or_else(|| p.get("action")).cloned().unwrap_or(Value::Null),
                    a => a.clone(),
                };
                pending.insert(
                    id.to_string(),
                    Call { tool: tool_label(name), at, chars: text_len(p), input: input_summary(&args), command: codex_call_command(name, &args) },
                );
            }
            Some("response_item") if kind.ends_with("_output") => {
                let Some(call) = p["call_id"].as_str().and_then(|id| pending.remove(id)) else { continue };
                if at.or(call.at).is_none_or(|t| t < since) {
                    continue;
                }
                // Older shells answer with JSON: {"output": …, "metadata": {"exit_code", "duration_seconds"}}.
                let raw = text_of(&p["output"]);
                let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                let output = json["output"].as_str().map_or(raw.clone(), String::from);
                let code = json["metadata"]["exit_code"].as_i64().or_else(|| exit_code(&raw));
                let failed = code.is_some_and(|c| c != 0) || p["status"] == "failed" || p["success"] == false;
                let out = Outcome { at, failed, ms: codex_ms(&json["metadata"]["duration_seconds"]), chars: text_len(p), error: if failed { output } else { String::new() } };
                acc.record("codex", call, out, cwd.as_deref());
                any = true;
            }
            Some("event_msg") => {
                // Commands Codex ran, whichever tool ran them.
                let item = if kind == "item_completed" && p["item"]["type"] == "CommandExecution" {
                    &p["item"]
                } else if kind == "exec_command_end" {
                    p
                } else {
                    continue;
                };
                if at.is_none_or(|t| t < since) {
                    continue;
                }
                let Some(cmd) = command_of(&item["command"]) else { continue };
                let code = item["exit_code"].as_i64().or_else(|| item["exit_code"].as_str().and_then(|s| s.parse().ok()));
                let output = item["aggregated_output"].as_str().or_else(|| item["formatted_output"].as_str()).unwrap_or_default();
                acc.command(&cmd, code.is_some_and(|c| c != 0), codex_ms(&item["duration"]), output.len() as u64);
            }
            _ => {}
        }
    }
    acc.sessions += any as u64;
}

type Parser = fn(&str, i64, &mut Acc);

// Command ----------------------------------------------------------------------------------------

/// Tool calls and commands over the logs of the last `days`.
pub fn analyze(since: i64) -> Report {
    let mut acc = Acc::default();
    let sources: [(_, Parser); 2] = [(claude_projects(), claude_file), (codex_sessions(), codex_file)];
    for (root, parse) in sources {
        let Some(root) = root else { continue };
        for path in jsonl_files(&root, since) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                parse(&text, since, &mut acc);
            }
        }
    }
    acc.report()
}

#[tauri::command]
pub async fn usage_tool_stats(days: i64) -> Result<Report, String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let since = now - days.clamp(1, crate::usage_history::KEEP_DAYS) * 24 * 60 * 60 * 1000;
    tauri::async_runtime::spawn_blocking(move || analyze(since)).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(parse: fn(&str, i64, &mut Acc), lines: &[Value]) -> Report {
        let text = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        let mut acc = Acc::default();
        parse(&text, 0, &mut acc);
        acc.report()
    }

    #[test]
    fn names_programs() {
        assert_eq!(program("git status"), "git");
        assert_eq!(program("cd /a && FOO=1 npm test"), "npm");
        assert_eq!(program("/usr/bin/python3 -I x.py | head"), "python3");
        assert_eq!(program("sudo -n ls"), "ls");
        assert_eq!(program("cd /a"), "cd");
        assert_eq!(program("(cargo test)"), "cargo");
    }

    #[test]
    fn reads_exit_codes() {
        assert_eq!(exit_code("Exit code 1\nboom"), Some(1));
        assert_eq!(exit_code("Process exited with code 0\n"), Some(0));
        assert_eq!(exit_code(r#"{"chunk_id":"x","exit_code":2}"#), Some(2));
        assert_eq!(exit_code("all good"), None);
    }

    #[test]
    fn counts_claude_tool_calls() {
        let r = run(
            claude_file,
            &[
                serde_json::json!({"type":"assistant","timestamp":"2026-10-20T10:00:00.000Z","cwd":"/w","message":{"content":[
                    {"type":"tool_use","id":"a","name":"Bash","input":{"command":"cd /w && cargo test"}},
                    {"type":"tool_use","id":"b","name":"mcp__winfer__list_panes","input":{}}]}}),
                serde_json::json!({"type":"user","timestamp":"2026-10-20T10:00:02.500Z","message":{"content":[
                    {"type":"tool_result","tool_use_id":"a","is_error":true,"content":"Exit code 101\nfailed"}]}}),
                serde_json::json!({"type":"user","timestamp":"2026-10-20T10:00:00.100Z","message":{"content":[
                    {"type":"tool_result","tool_use_id":"b","content":[{"type":"text","text":"[]"}]}]}}),
            ],
        );
        assert_eq!((r.calls, r.failed, r.sessions), (2, 1, 1));
        let bash = r.tools.iter().find(|s| s.name == "Bash").unwrap();
        assert_eq!((bash.failed, bash.duration_ms), (1, 2500));
        assert!(r.tools.iter().any(|s| s.name == "list_panes" && s.duration_ms == 100));
        assert_eq!(r.commands[0].name, "cargo");
        let f = &r.failures[0];
        assert_eq!((f.tool.as_str(), f.input.as_str(), f.cwd.as_deref()), ("Bash", "cd /w && cargo test", Some("/w")));
        assert!(f.error.starts_with("Exit code 101"));
    }

    #[test]
    fn counts_codex_tool_calls_and_commands() {
        let r = run(
            codex_file,
            &[
                serde_json::json!({"timestamp":"2026-10-20T10:00:00.000Z","type":"turn_context","payload":{"cwd":"/w","model":"gpt"}}),
                serde_json::json!({"timestamp":"2026-10-20T10:00:00.000Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"command\":[\"bash\",\"-lc\",\"npm test\"]}"}}),
                serde_json::json!({"timestamp":"2026-10-20T10:00:05.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"1 failing\",\"metadata\":{\"exit_code\":1,\"duration_seconds\":4.2}}"}}),
                serde_json::json!({"timestamp":"2026-10-20T10:00:06.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","command":["/bin/zsh","-lc","git status"],"exit_code":"0","duration":{"secs":1,"nanos":500000000},"aggregated_output":"clean"}}}),
            ],
        );
        assert_eq!((r.calls, r.failed), (1, 1));
        assert_eq!(r.tools[0].duration_ms, 4200);
        assert_eq!(r.failures[0].error, "1 failing");
        assert_eq!(r.failures[0].input, "npm test");
        let names: Vec<_> = r.commands.iter().map(|s| (s.name.as_str(), s.failed, s.duration_ms)).collect();
        assert_eq!(names, vec![("git", 0, 1500), ("npm", 1, 4200)]);
    }

    #[test]
    #[ignore]
    fn tool_stats_on_this_machine() {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64 - 24 * 3_600_000;
        let r = analyze(since);
        eprintln!("{} calls, {} failed, {} sessions", r.calls, r.failed, r.sessions);
        for s in r.tools.iter().take(10) {
            eprintln!("{:<20} {:>6} {:>4} {:>9} tok {:>8} ms", s.name, s.calls, s.failed, s.tokens, s.duration_ms);
        }
        for s in r.commands.iter().take(10) {
            eprintln!("$ {:<18} {:>6} {:>4}", s.name, s.calls, s.failed);
        }
    }
}
