//! Micro-compaction for the CLI backends. The ChatGPT backend owns its history and trims it in
//! memory (chatgpt.rs); Claude Code and Codex keep theirs in a session file and offer no way to
//! trim it. Both rebuild the conversation from that file when a session is resumed, so between
//! turns the bridge stops the CLI, trims old tool output in the file and resumes the session in a
//! fresh process. Turns older than the last [`KEEP_TURNS`] lose long tool output and file
//! contents in tool inputs; the model can run a tool again if it needs one.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harness::event::Usage;

/// Recent user turns left whole.
const KEEP_TURNS: usize = 2;
/// Tool output and inputs at most this long are left alone.
const MIN_LEN: usize = 300;
/// What's kept of a trimmed output.
const HEAD: usize = 200;
const NOTE: &str = " …[trimmed; run the tool again if needed]";
/// Restarting the CLI takes a moment and its prompt cache starts over, so a trim has to save at
/// least this much (≈6k tokens) to be worth it.
pub const MIN_SAVING: usize = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Claude Code's `~/.claude/projects/<project>/<session>.jsonl`.
    Claude,
    /// Codex's `~/.codex/sessions/YYYY/MM/DD/rollout-…-<thread>.jsonl`.
    Codex,
}

/// Whether the context is past `percent` of the window (0 = never).
pub fn over(usage: &Usage, percent: u8) -> bool {
    percent > 0 && usage.window > 0 && usage.context * 100 > usage.window * percent as u64
}

/// Whether to micro-compact: past `trim_at`, but not yet at `compact_at` (that summarizes instead).
pub fn due(usage: &Usage, trim_at: u8, compact_at: u8) -> bool {
    over(usage, trim_at) && !over(usage, compact_at)
}

/// Claude Code's session file for `session`.
pub fn claude_transcript(session: &str) -> Option<PathBuf> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    let name = format!("{session}.jsonl");
    std::fs::read_dir(config.join("projects")).ok()?.flatten().map(|d| d.path().join(&name)).find(|p| p.is_file())
}

/// Codex's rollout file for `thread` (newest day folders first).
pub fn codex_rollout(thread: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
    find_rollout(&home.join("sessions"), &format!("-{thread}.jsonl"), 3)
}

fn find_rollout(dir: &Path, suffix: &str, depth: u8) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort_unstable_by(|a, b| b.cmp(a));
    entries.into_iter().find_map(|p| {
        if depth == 0 {
            p.file_name()?.to_str()?.ends_with(suffix).then_some(p)
        } else if p.is_dir() {
            find_rollout(&p, suffix, depth - 1)
        } else {
            None
        }
    })
}

/// Trims the session file at `path` if that saves at least `min_saving` bytes, or (with `apply`
/// false) only says how much it would. Returns the bytes saved, 0 when the file is left alone.
pub fn trim_file(path: &Path, format: Format, min_saving: usize, apply: bool) -> io::Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let (lines, saved) = trim_lines(&text, format);
    if saved < min_saving.max(1) {
        return Ok(0);
    }
    if apply {
        let tmp = path.with_extension("jsonl.trim");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(saved)
}

/// The file's lines with the old turns trimmed (untouched lines stay byte for byte), and the
/// bytes saved.
fn trim_lines(text: &str, format: Format) -> (Vec<String>, usize) {
    let raw: Vec<&str> = text.lines().collect();
    let parsed: Vec<Option<Value>> = raw.iter().map(|l| serde_json::from_str(l).ok()).collect();
    let starts: Vec<usize> = parsed.iter().enumerate().filter(|(_, v)| v.as_ref().is_some_and(|v| turn_start(v, format))).map(|(n, _)| n).collect();
    let cut = starts.len().checked_sub(KEEP_TURNS).and_then(|k| starts.get(k)).copied().unwrap_or(0);
    let mut saved = 0;
    let lines = raw
        .iter()
        .zip(parsed)
        .enumerate()
        .map(|(n, (line, v))| match v {
            Some(mut v) if n < cut => {
                if !trim_line(&mut v, format) {
                    return line.to_string();
                }
                let out = v.to_string();
                saved += line.len().saturating_sub(out.len());
                out
            }
            _ => line.to_string(),
        })
        .collect();
    (lines, saved)
}

/// Whether the line starts a user turn.
fn turn_start(v: &Value, format: Format) -> bool {
    match format {
        // The user's own messages: not tool results, meta messages or a subagent's.
        Format::Claude => {
            let content = &v["message"]["content"];
            v["type"] == "user"
                && v["isMeta"] != true
                && v["isSidechain"] != true
                && (content.is_string() || content.as_array().is_some_and(|c| !c.is_empty() && c.iter().all(|b| b["type"] != "tool_result")))
        }
        Format::Codex => v["type"] == "event_msg" && v["payload"]["type"] == "task_started",
    }
}

/// Trims one line in place; whether it changed.
fn trim_line(v: &mut Value, format: Format) -> bool {
    let mut changed = false;
    match format {
        Format::Claude => {
            let Some(blocks) = v["message"]["content"].as_array_mut() else { return false };
            for b in blocks {
                match b["type"].as_str() {
                    Some("tool_result") => changed |= trim_output(&mut b["content"], false),
                    Some("tool_use") => {
                        changed |= trim_inputs(&mut b["input"]);
                        if let Some(edits) = b["input"]["edits"].as_array_mut() {
                            for e in edits {
                                changed |= trim_inputs(e);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Format::Codex => {
            if v["type"] != "response_item" {
                return false;
            }
            let p = &mut v["payload"];
            match p["type"].as_str() {
                Some("function_call_output" | "custom_tool_call_output") => changed = trim_output(&mut p["output"], true),
                Some("function_call") => {
                    if let Ok(mut args) = serde_json::from_str::<Value>(p["arguments"].as_str().unwrap_or("")) {
                        if trim_inputs(&mut args) {
                            p["arguments"] = json!(args.to_string());
                            changed = true;
                        }
                    }
                }
                // A freeform call's input is the patch or script itself.
                Some("custom_tool_call") => {
                    if let Some(input) = p["input"].as_str().filter(|s| s.len() > MIN_LEN) {
                        p["input"] = json!(format!("{}{NOTE}", head(input)));
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }
    changed
}

/// Cuts a long tool output: a string, or a list of content parts (text, images). Codex wants a
/// list to stay a list (`as_parts`).
fn trim_output(out: &mut Value, as_parts: bool) -> bool {
    let text = match &*out {
        Value::String(s) if s.len() > MIN_LEN => s.clone(),
        Value::Array(parts) if out.to_string().len() > MIN_LEN => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return false,
    };
    let cut = format!("{}{NOTE}", head(&text));
    *out = if as_parts && out.is_array() { json!([{ "type": "input_text", "text": cut }]) } else { json!(cut) };
    true
}

/// Replaces file contents in a tool call's input (writes, edits, patches).
fn trim_inputs(input: &mut Value) -> bool {
    let mut changed = false;
    for k in ["content", "old", "new", "old_string", "new_string", "new_source", "patch", "input"] {
        if input[k].as_str().is_some_and(|s| s.len() > MIN_LEN) {
            input[k] = json!("[trimmed]");
            changed = true;
        }
    }
    changed
}

fn head(s: &str) -> &str {
    &s[..s.floor_char_boundary(HEAD)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(c: char) -> String {
        c.to_string().repeat(5000)
    }

    fn claude_session() -> String {
        let user = |t: &str| json!({ "type": "user", "message": { "role": "user", "content": t } });
        let call = |id: &str| json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": id, "name": "Write", "input": { "file_path": "a", "content": big('w') } }] } });
        let result = |id: &str| json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": id, "content": big('r') }] }, "toolUseResult": { "stdout": "x" } });
        [user("one"), call("t1"), result("t1"), user("two"), call("t2"), result("t2"), user("three"), call("t3"), result("t3")]
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn claude_trims_turns_before_the_last_two() {
        let (lines, saved) = trim_lines(&claude_session(), Format::Claude);
        assert!(saved > 9000, "{saved}");
        let v: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(v[1]["message"]["content"][0]["input"]["content"], "[trimmed]");
        assert!(v[2]["message"]["content"][0]["content"].as_str().unwrap().ends_with(NOTE));
        // The last two turns stay whole, and so does what Claude Code doesn't send to the model.
        assert_eq!(v[5]["message"]["content"][0]["content"].as_str().unwrap().len(), 5000);
        assert_eq!(v[8]["message"]["content"][0]["content"].as_str().unwrap().len(), 5000);
        assert_eq!(v[2]["toolUseResult"]["stdout"], "x");
        // Trimming again saves nothing.
        assert_eq!(trim_lines(&lines.join("\n"), Format::Claude).1, 0);
    }

    #[test]
    fn codex_trims_outputs_keeping_their_shape() {
        let turn = |n: u32| {
            vec![
                json!({ "type": "event_msg", "payload": { "type": "task_started" } }),
                json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "call_id": format!("c{n}"), "input": big('p') } }),
                json!({ "type": "response_item", "payload": { "type": "custom_tool_call_output", "call_id": format!("c{n}"), "output": [{ "type": "input_text", "text": big('o') }] } }),
                json!({ "type": "response_item", "payload": { "type": "function_call_output", "call_id": format!("f{n}"), "output": big('s') } }),
            ]
        };
        let text = (1..=3).flat_map(turn).map(|v| v.to_string()).collect::<Vec<_>>().join("\n");
        let (lines, saved) = trim_lines(&text, Format::Codex);
        assert!(saved > 14000, "{saved}");
        let v: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(v[1]["payload"]["input"].as_str().unwrap().ends_with(NOTE));
        assert!(v[2]["payload"]["output"][0]["text"].as_str().unwrap().ends_with(NOTE));
        assert!(v[3]["payload"]["output"].as_str().unwrap().ends_with(NOTE));
        assert_eq!(v[6]["payload"]["output"][0]["text"].as_str().unwrap().len(), 5000);
    }

    /// Runs a real CLI through a micro-compact (costs a few cents):
    /// `cargo test --lib microcompact::tests::live -- --ignored --nocapture`
    fn live(kind: super::super::Kind, model: &str) {
        use crate::harness::event::{Cmd, Ev, Mode, UserMsg};
        let dir = std::env::temp_dir().join(format!("winfer-mc-live-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        for (n, name) in ["a.txt", "b.txt", "c.txt"].iter().enumerate() {
            let mut text: String = (0..150).map(|i| format!("{name} line {i}: lorem ipsum dolor sit amet consectetur adipiscing elit\n")).collect();
            if n == 2 {
                text.push_str("The last line says ORANGE-GIRAFFE-77.\n");
            }
            std::fs::write(dir.join(name), text).unwrap();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let opts = super::super::Opts {
            cwd: dir.clone(),
            model: model.into(),
            effort: None,
            mode: Mode::Yolo,
            resume: None,
            system: None,
            mcp: Default::default(),
            allowed_commands: Vec::new(),
            trim_at: 1,
            compact_at: 0,
            lean: true,
            skills: crate::harness::claude_skills::LeanSkills::Only(Vec::new()),
            bg_owner: String::new(),
            sub_model: None,
            sub_effort: None,
            account: None,
        };
        let h = super::super::start(kind, opts, tx);
        let wait = |until: &dyn Fn(&Ev) -> bool| -> Vec<Ev> {
            let mut seen = Vec::new();
            while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_secs(240)) {
                if let Ev::Exited(e) = &ev {
                    panic!("exited: {e}");
                }
                let done = until(&ev);
                seen.push(ev);
                if done {
                    return seen;
                }
            }
            panic!("timed out; saw {seen:?}");
        };
        let turn = |text: &str| -> String {
            h.send(Cmd::Send(UserMsg { text: text.into(), images: Vec::new(), skill: None }));
            let evs = wait(&|e| matches!(e, Ev::TurnDone));
            evs.iter().filter_map(|e| if let Ev::Text { text, .. } = e { Some(text.clone()) } else { None }).collect::<Vec<_>>().join("\n")
        };
        turn("Show me the full contents of a.txt, b.txt and c.txt: one tool call per file (read the whole file, or cat it). Then reply only OK, without repeating them.");
        turn("Reply only OK.");
        turn("Reply only OK.");
        // The third turn pushed the first out of the two kept whole.
        let evs = wait(&|e| matches!(e, Ev::Notice(n) if n.starts_with("Micro-compacted")));
        eprintln!("{:?}", evs.last());
        let answer = turn("Without using any tools: 1) which files did you read earlier? 2) what exactly does the last line of c.txt say? If you can't see it in our conversation, answer 2) with UNKNOWN.");
        eprintln!("{answer}");
        assert!(answer.contains("c.txt") && answer.contains("UNKNOWN") && !answer.contains("ORANGE"), "{answer}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore]
    fn live_claude() {
        live(super::super::Kind::Claude, "haiku");
    }

    #[test]
    #[ignore]
    fn live_codex() {
        live(super::super::Kind::Codex, "");
    }

    #[test]
    fn small_savings_leave_the_file_alone() {
        let dir = std::env::temp_dir().join(format!("winfer-mc-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(&path, claude_session()).unwrap();
        assert_eq!(trim_file(&path, Format::Claude, 1_000_000, true).unwrap(), 0);
        assert!(trim_file(&path, Format::Claude, 1000, false).unwrap() > 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), claude_session());
        assert!(trim_file(&path, Format::Claude, 1000, true).unwrap() > 0);
        assert_ne!(std::fs::read_to_string(&path).unwrap(), claude_session());
        let _ = std::fs::remove_dir_all(dir);
    }
}
