//! Context handoff when switching between backends/sessions inside a `winfer code` conversation.
//!
//! Each CLI/API backend owns its own native conversation thread (`backend_session`). When the user
//! switches to a different backend mid-conversation, the new backend can't resume the old one's
//! native session id. Instead of dropping the conversation and starting fresh, we package a
//! structured, token-budgeted digest of the session transcript, active plan, and workspace state
//! and attach it to the first message sent to the new backend.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::event::{Todo, UserMsg};
use super::session::Entry;

/// Soft byte budget for the full `<context_handoff>` block (~12k tokens).
const MAX_DIGEST_BYTES: usize = 48_000;
/// Number of trailing conversation blocks kept at full (higher) per-block fidelity.
const RECENT_BLOCKS: usize = 6;
const RECENT_BLOCK_CHARS: usize = 8_000;
const OLDER_BLOCK_CHARS: usize = 1_600;
const TOOL_LINE_CHARS: usize = 200;
const MAX_STATUS_LINES: usize = 30;

/// Inputs needed to build a handoff digest for a new backend.
pub struct HandoffContext<'a> {
    pub from_spec: &'a str,
    pub from_actual: Option<&'a str>,
    pub to_spec: &'a str,
    pub cwd: &'a Path,
    pub branch: Option<&'a str>,
    pub git_status: Option<&'a str>,
    pub todos: &'a [Todo],
    pub transcript: &'a [Entry],
    /// Optional short error/outcome excerpts for failed or notable tool entries, keyed by
    /// `transcript` index.
    pub tool_notes: &'a HashMap<usize, String>,
}

/// True when the session has actual conversation turns worth handing over.
pub fn has_context(transcript: &[Entry]) -> bool {
    transcript.iter().any(|e| matches!(e.role.as_str(), "user" | "assistant"))
}

/// Fallback plain-text label stored on a `"handoff"` transcript entry.
pub fn fallback_text(from_spec: &str, to_spec: &str) -> String {
    format!("Context handoff: {from_spec} → {to_spec}")
}

/// Short uncommitted git status for `cwd`, capped to [`MAX_STATUS_LINES`] lines.
pub fn git_status_short(cwd: &Path) -> Option<String> {
    let out = crate::env::no_window(
        std::process::Command::new("git")
            .args(["status", "--short", "--untracked-files=normal"])
            .current_dir(cwd),
    )
    .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let mut text = lines.iter().take(MAX_STATUS_LINES).copied().collect::<Vec<_>>().join("\n");
    if lines.len() > MAX_STATUS_LINES {
        text.push_str(&format!("\n... and {} more", lines.len() - MAX_STATUS_LINES));
    }
    Some(text)
}

struct Block {
    is_user: bool,
    is_tool: bool,
    raw: String,
    edited_files: Vec<String>,
}

/// Builds a `<context_handoff>` block from `ctx`, or `None` if there is no prior conversation.
pub fn build_digest(ctx: &HandoffContext<'_>) -> Option<String> {
    if !has_context(ctx.transcript) {
        return None;
    }

    let from_desc = match ctx.from_actual.filter(|a| !a.is_empty() && !ctx.from_spec.ends_with(a)) {
        Some(actual) => format!("{} ({actual})", ctx.from_spec),
        None => ctx.from_spec.to_string(),
    };
    let branch_attr = ctx
        .branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| format!(" branch=\"{}\"", escape_attr(b)))
        .unwrap_or_default();

    let mut header = format!(
        "<context_handoff from=\"{}\" to=\"{}\" cwd=\"{}\"{branch_attr}>\n\
         This conversation was seamlessly handed off to you from another model/session in the same workspace.\n\
         Continue directly from the prior conversation and current workspace state. Do not re-introduce yourself, apologize for, or announce the handoff unless the user asks.\n",
        escape_attr(&from_desc),
        escape_attr(ctx.to_spec),
        escape_attr(&ctx.cwd.to_string_lossy()),
    );

    if let Some(status) = ctx.git_status.map(str::trim_end).filter(|s| !s.trim().is_empty()) {
        header.push_str("\n## Uncommitted workspace changes\n");
        header.push_str(status);
        header.push('\n');
    }

    if !ctx.todos.is_empty() {
        header.push_str("\n## Current plan\n");
        for t in ctx.todos {
            let mark = match t.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[~]",
                "skipped" => "[skipped]",
                "failed" => "[failed]",
                _ => "[ ]",
            };
            header.push_str(&format!("- {mark} {}\n", t.content.trim()));
        }
    }

    let blocks = group_blocks(ctx.transcript, ctx.tool_notes);
    if blocks.is_empty() {
        return None;
    }

    let total = blocks.len();
    let formatted: Vec<String> = blocks
        .iter()
        .enumerate()
        .map(|(idx, b)| {
            let recent = total.saturating_sub(idx) <= RECENT_BLOCKS;
            let limit = if recent {
                RECENT_BLOCK_CHARS
            } else if b.is_tool {
                1_200
            } else {
                OLDER_BLOCK_CHARS
            };
            clip_middle(&b.raw, limit)
        })
        .collect();

    let footer = "\n</context_handoff>";
    let section_head = "\n## Conversation so far\n";
    let fixed_bytes = header.len() + section_head.len() + footer.len();
    let body_budget = MAX_DIGEST_BYTES.saturating_sub(fixed_bytes).max(4_000);

    let body = fit_blocks(&blocks, &formatted, body_budget);
    Some(format!("{header}{section_head}{body}{footer}"))
}

/// Attaches `digest` to `msg` without breaking `/skill` invocations.
pub fn wrap_user_message(msg: &mut UserMsg, digest: &str) {
    if digest.is_empty() {
        return;
    }
    if msg.skill.is_some() {
        if msg.text.trim().is_empty() {
            msg.text = digest.to_string();
        } else {
            msg.text = format!("{}\n\n{digest}", msg.text);
        }
    } else if msg.text.trim().is_empty() {
        msg.text = format!("{digest}\n\n(See attached image or file.)");
    } else {
        msg.text = format!("{digest}\n\n{}", msg.text);
    }
}

fn group_blocks(transcript: &[Entry], tool_notes: &HashMap<usize, String>) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut tool_lines: Vec<String> = Vec::new();
    let mut tool_files: Vec<String> = Vec::new();

    let flush_tools = |blocks: &mut Vec<Block>, lines: &mut Vec<String>, files: &mut Vec<String>| {
        if lines.is_empty() {
            return;
        }
        blocks.push(Block {
            is_user: false,
            is_tool: true,
            raw: format!("[Tools]\n{}", lines.join("\n")),
            edited_files: std::mem::take(files),
        });
        lines.clear();
    };

    for (idx, entry) in transcript.iter().enumerate() {
        match entry.role.as_str() {
            "user" => {
                flush_tools(&mut blocks, &mut tool_lines, &mut tool_files);
                let text = entry.text.trim();
                if !text.is_empty() {
                    blocks.push(Block {
                        is_user: true,
                        is_tool: false,
                        raw: format!("[User]\n{text}"),
                        edited_files: Vec::new(),
                    });
                }
            }
            "assistant" => {
                flush_tools(&mut blocks, &mut tool_lines, &mut tool_files);
                let text = entry.text.trim();
                if !text.is_empty() {
                    blocks.push(Block {
                        is_user: false,
                        is_tool: false,
                        raw: format!("[Assistant]\n{text}"),
                        edited_files: Vec::new(),
                    });
                }
            }
            "tool" => {
                let (name, title) = entry.text.split_once('\t').unwrap_or((entry.text.as_str(), ""));
                let name = name.trim();
                let title = title.trim();
                if is_edit_tool(name) && !title.is_empty() {
                    tool_files.push(title.to_string());
                }
                let status = if entry.state.as_deref() == Some("failed") { " (failed)" } else { "" };
                let note = tool_notes
                    .get(&idx)
                    .map(|n| n.trim())
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" — {}", single_line(n, 140)))
                    .unwrap_or_default();
                let line = if title.is_empty() {
                    format!("- {name}{status}{note}")
                } else {
                    format!("- {name} {}{status}{note}", single_line(title, TOOL_LINE_CHARS))
                };
                tool_lines.push(line);
            }
            // Notices and prior handoff markers are UI chrome, not conversation state.
            _ => {}
        }
    }
    flush_tools(&mut blocks, &mut tool_lines, &mut tool_files);
    blocks
}

fn fit_blocks(blocks: &[Block], formatted: &[String], budget: usize) -> String {
    let total_bytes: usize = formatted.iter().map(|s| s.len() + 2).sum();
    if total_bytes <= budget {
        return formatted.join("\n\n");
    }

    // Always preserve the first user goal (if any) and pack as many trailing blocks as fit.
    let first_user = blocks.iter().position(|b| b.is_user);
    let mut keep_head = false;
    let mut used = 0usize;
    let reserve_note = 256usize;

    if let Some(hi) = first_user {
        if hi + 1 < blocks.len() && formatted[hi].len() + reserve_note < budget / 2 {
            keep_head = true;
            used += formatted[hi].len() + 2;
        }
    }

    let head_end = if keep_head { first_user.unwrap() + 1 } else { 0 };
    let mut tail_start = blocks.len();
    for i in (head_end..blocks.len()).rev() {
        let need = formatted[i].len() + 2;
        if tail_start < blocks.len() && used + need + reserve_note > budget {
            break;
        }
        used += need;
        tail_start = i;
    }

    let mut out: Vec<String> = Vec::new();
    if keep_head {
        out.push(formatted[first_user.unwrap()].clone());
    }

    let omitted_range = head_end..tail_start;
    if !omitted_range.is_empty() {
        let omitted_count = omitted_range.len();
        let mut seen = HashSet::new();
        let mut touched: Vec<&str> = Vec::new();
        for b in &blocks[omitted_range] {
            for f in &b.edited_files {
                if seen.insert(f.as_str()) {
                    touched.push(f.as_str());
                }
            }
        }
        let mut marker = format!("[... {omitted_count} earlier conversation block{} omitted for length ...]", if omitted_count == 1 { "" } else { "s" });
        if !touched.is_empty() {
            let shown: Vec<&str> = touched.iter().take(20).copied().collect();
            marker.push_str(&format!("\nFiles modified in omitted turns: {}", shown.join(", ")));
            if touched.len() > 20 {
                marker.push_str(&format!(" (+{} more)", touched.len() - 20));
            }
        }
        out.push(marker);
    }

    for s in &formatted[tail_start..] {
        out.push(s.clone());
    }

    out.join("\n\n")
}

fn is_edit_tool(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "edit" | "write" | "multiedit" | "notebookedit" | "apply_patch")
}

fn escape_attr(s: &str) -> String {
    s.replace('"', "'").replace('\n', " ").replace('\r', "")
}

fn single_line(s: &str, max_chars: usize) -> String {
    let clean: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match clean.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", &clean[..i]),
        None => clean,
    }
}

fn clip_middle(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars || max_chars < 64 {
        return text.to_string();
    }
    let head_chars = max_chars * 2 / 3;
    let tail_chars = max_chars.saturating_sub(head_chars + 24);
    let head_end = text.char_indices().nth(head_chars).map(|(i, _)| i).unwrap_or(text.len());
    let tail_start = text.char_indices().nth(count.saturating_sub(tail_chars)).map(|(i, _)| i).unwrap_or(text.len());
    format!("{}\n[... truncated ...]\n{}", text[..head_end].trim_end(), text[tail_start..].trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(role: &str, text: &str) -> Entry {
        Entry { role: role.into(), text: text.into(), ..Default::default() }
    }

    #[test]
    fn empty_or_notice_only_transcript_has_no_context() {
        assert!(!has_context(&[]));
        assert!(!has_context(&[entry("notice", "hello")]));
        let ctx = HandoffContext {
            from_spec: "claude/fable",
            from_actual: Some("claude-fable-5-2"),
            to_spec: "agy/gemini-4-neon",
            cwd: Path::new("/tmp/repo"),
            branch: Some("main"),
            git_status: None,
            todos: &[],
            transcript: &[entry("notice", "ready")],
            tool_notes: &HashMap::new(),
        };
        assert!(build_digest(&ctx).is_none());
    }

    #[test]
    fn builds_structured_digest_with_tools_plan_and_git_status() {
        let mut failed_tool = entry("tool", "Bash\tcargo test");
        failed_tool.state = Some("failed".into());
        let transcript = vec![
            entry("user", "Fix the login redirect loop"),
            entry("tool", "Read\tsrc/auth.rs"),
            entry("tool", "Edit\tsrc/auth.rs"),
            failed_tool,
            entry("assistant", "I updated src/auth.rs, one test still fails on session cookie."),
            entry("handoff", "Context handoff: claude/fable → agy/gemini-4-neon"),
        ];
        let mut notes = HashMap::new();
        notes.insert(3, "assertion failed: cookie missing".to_string());
        let todos = vec![
            Todo { content: "Trace redirect".into(), status: "completed".into() },
            Todo { content: "Fix cookie test".into(), status: "in_progress".into() },
        ];
        let ctx = HandoffContext {
            from_spec: "claude/fable",
            from_actual: Some("claude-fable-5-2"),
            to_spec: "agy/gemini-4-neon",
            cwd: Path::new("/tmp/repo"),
            branch: Some("fix/login"),
            git_status: Some(" M src/auth.rs"),
            todos: &todos,
            transcript: &transcript,
            tool_notes: &notes,
        };
        let digest = build_digest(&ctx).unwrap();
        assert!(digest.starts_with("<context_handoff from=\"claude/fable (claude-fable-5-2)\" to=\"agy/gemini-4-neon\" cwd=\"/tmp/repo\" branch=\"fix/login\">"));
        assert!(digest.contains("## Uncommitted workspace changes\n M src/auth.rs"));
        assert!(digest.contains("- [x] Trace redirect"));
        assert!(digest.contains("- [~] Fix cookie test"));
        assert!(digest.contains("[User]\nFix the login redirect loop"));
        assert!(digest.contains("- Read src/auth.rs"));
        assert!(digest.contains("- Bash cargo test (failed) — assertion failed: cookie missing"));
        assert!(digest.contains("[Assistant]\nI updated src/auth.rs"));
        assert!(digest.ends_with("</context_handoff>"));
    }

    #[test]
    fn trims_middle_turns_while_keeping_first_goal_and_latest_turns() {
        let mut transcript = vec![entry("user", "ORIGINAL_GOAL_MARKER: build handoff")];
        for i in 0..80 {
            transcript.push(entry("tool", &format!("Edit\tsrc/file_{i}.rs")));
            transcript.push(entry("assistant", &format!("Turn {i}: {}", "x".repeat(1200))));
            transcript.push(entry("user", &format!("Follow-up {i}: {}", "y".repeat(600))));
        }
        transcript.push(entry("assistant", "LATEST_REPLY_MARKER: almost done"));
        let ctx = HandoffContext {
            from_spec: "claude/opus",
            from_actual: None,
            to_spec: "codex/gpt-6.1-sol",
            cwd: Path::new("/tmp/repo"),
            branch: None,
            git_status: None,
            todos: &[],
            transcript: &transcript,
            tool_notes: &HashMap::new(),
        };
        let digest = build_digest(&ctx).unwrap();
        assert!(digest.len() <= MAX_DIGEST_BYTES + 1024);
        assert!(digest.contains("ORIGINAL_GOAL_MARKER"));
        assert!(digest.contains("LATEST_REPLY_MARKER"));
        assert!(digest.contains("omitted for length"));
        assert!(digest.contains("Files modified in omitted turns:"));
    }

    #[test]
    fn wraps_regular_and_skill_messages() {
        let mut normal = UserMsg { text: "Continue please".into(), images: Vec::new(), skill: None };
        wrap_user_message(&mut normal, "<context_handoff>hi</context_handoff>");
        assert_eq!(normal.text, "<context_handoff>hi</context_handoff>\n\nContinue please");

        let mut skill = UserMsg { text: "src/main.rs".into(), images: Vec::new(), skill: Some("review".into()) };
        wrap_user_message(&mut skill, "<context_handoff>hi</context_handoff>");
        assert_eq!(skill.text, "src/main.rs\n\n<context_handoff>hi</context_handoff>");

        let mut image_only = UserMsg { text: String::new(), images: vec![PathBuf::from("/tmp/a.png")], skill: None };
        wrap_user_message(&mut image_only, "<context_handoff>hi</context_handoff>");
        assert!(image_only.text.starts_with("<context_handoff>hi</context_handoff>\n\n"));
    }

    #[test]
    fn clip_middle_handles_multibyte_chars() {
        let s = "🚀→—日本語".repeat(50);
        let clipped = clip_middle(&s, 100);
        assert!(clipped.contains("[... truncated ...]"));
    }
}
