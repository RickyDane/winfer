//! The composer's slash commands: what `tui::app` does for each, mapped to the chat pane
//! (port of `features/code/commands.ts`).

use super::protocol::*;
use super::state::{CodeState, Item, Sheet};
use std::sync::atomic::{AtomicU64, Ordering};

static BTW: AtomicU64 = AtomicU64::new(0);

/// What a command needs from the view beyond changing the state.
#[derive(Debug, PartialEq)]
pub enum Effect {
    Done,
    Copy(String),
    /// Plan sidebar: Some(on/off), or None to toggle.
    Sidebar(Option<bool>),
}

/// `/name args` → the skill call, when `name` is a known skill and not a built-in command (those win).
pub fn skill_call(text: &str, skills: &[String]) -> Option<(String, String)> {
    let t = text.trim();
    if !t.starts_with('/') || t.starts_with("//") {
        return None;
    }
    let mut parts = t.split_whitespace();
    let name = parts.next()?[1..].to_string();
    if name.is_empty() || COMMANDS.iter().any(|(c, _)| c[1..] == name) || !skills.contains(&name) {
        return None;
    }
    Some((name, parts.collect::<Vec<_>>().join(" ")))
}

/// Runs `text` if it's a command; None means it's a message to send.
pub fn run_command(text: &str, s: &mut CodeState) -> Option<Effect> {
    let t = text.trim();
    if !t.starts_with('/') {
        return None;
    }
    let mut parts = t.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let arg = parts.collect::<Vec<_>>().join(" ");
    match cmd {
        "/model" => s.set_sheet(Some(Sheet::Model)),
        "/subagent-model" | "/submodel" => {
            if matches!(arg.as_str(), "main" | "default" | "same") {
                s.send(Cmd::SetSubagent { model: None, effort: None });
            } else if !arg.is_empty() {
                let mut it = arg.split_whitespace();
                let m = it.next().unwrap_or("");
                // Drop a backend prefix: `claude/opus` → `opus`.
                let model = match m.split_once('/') {
                    Some((p, rest)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()) => rest,
                    _ => m,
                };
                let effort = it.next().filter(|e| *e != "default").map(str::to_string);
                s.send(Cmd::SetSubagent { model: Some(model.to_string()), effort });
            } else {
                s.set_sheet(Some(Sheet::SubagentModel));
            }
        }
        "/effort" => {
            if arg.is_empty() {
                s.set_sheet(Some(Sheet::Effort));
            } else if arg == "ultracode" || arg.starts_with("ultracode ") {
                let tail = arg.strip_prefix("ultracode").unwrap_or("").trim();
                let next = match tail {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => !s.ultracode,
                };
                if super::names::supports_ultracode(&s.model) {
                    s.send(Cmd::SetUltracode(next));
                } else {
                    s.notice("Ultracode is available for Claude Code, Codex, and Antigravity models.");
                }
            } else {
                s.send(Cmd::SetEffort(if arg == "default" { None } else { Some(arg) }));
            }
        }
        "/ultracode" => {
            if !super::names::supports_ultracode(&s.model) {
                s.notice("Ultracode is available for Claude Code, Codex, and Antigravity models.");
            } else {
                let next = match arg.as_str() {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => !s.ultracode,
                };
                s.send(Cmd::SetUltracode(next));
            }
        }
        "/mode" => match Mode::parse(&arg) {
            Some(m) => s.send(Cmd::SetMode(m)),
            None => s.set_sheet(Some(Sheet::Mode)),
        },
        "/new" | "/clear" | "/reset" => s.send(Cmd::New),
        "/resume" | "/sessions" => {
            s.send(Cmd::Sessions);
            s.set_sheet(Some(Sheet::Sessions));
        }
        "/btw" => {
            if arg.is_empty() {
                s.set_sheet(Some(Sheet::Btw));
            } else {
                s.send(Cmd::Btw { id: format!("btw-{}", BTW.fetch_add(1, Ordering::Relaxed) + 1), question: arg });
            }
        }
        "/compact" => {
            if s.busy { s.notice("Wait for the turn to finish.") } else { s.send(Cmd::Compact) }
        }
        "/compaction" | "/autocompact" => s.set_sheet(Some(Sheet::Compaction)),
        "/undo" | "/revert" => s.send(Cmd::Undo { files: true }),
        "/init" => s.submit(INIT_PROMPT, &[], None),
        "/review" => s.submit(REVIEW_PROMPT, &[], None),
        "/diff" => s.send(Cmd::Diff),
        "/cost" | "/usage" | "/context" => s.set_sheet(Some(Sheet::Usage)),
        "/copy" => {
            return Some(match s.items.iter().rev().find_map(|i| if let Item::Text { text, .. } = i { Some(text.clone()) } else { None }) {
                Some(text) => Effect::Copy(text),
                None => {
                    s.notice("Nothing to copy yet.");
                    Effect::Done
                }
            });
        }
        "/export" => {
            let md = s.export_markdown();
            s.send(Cmd::Export(md));
        }
        "/agents" | "/subagents" | "/tasks" | "/shells" => s.set_sheet(Some(Sheet::Agents)),
        "/mcp" => s.set_sheet(Some(Sheet::Mcp)),
        "/status" => s.set_sheet(Some(Sheet::Status)),
        "/login" => s.set_sheet(Some(Sheet::Login)),
        "/help" => s.set_sheet(Some(Sheet::Help)),
        "/sidebar" => {
            return Some(Effect::Sidebar(match arg.as_str() {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            }));
        }
        other => s.notice(format!("Unknown command {other} — try /help")),
    }
    Some(Effect::Done)
}

/// Commands matching what's typed after the slash, for the composer's popup.
#[cfg(test)]
pub fn matching(prefix: &str) -> Vec<(&'static str, &'static str)> {
    let p = prefix.trim_start_matches('/').to_lowercase();
    COMMANDS.iter().filter(|(c, _)| c[1..].starts_with(&p)).copied().collect()
}

/// Built-in commands and session skills matching what's typed after the slash, for the composer's popup.
pub fn matching_with_skills(prefix: &str, skills: &[String]) -> Vec<(String, String)> {
    let p = prefix.trim_start_matches('/').to_lowercase();
    let mut out: Vec<(String, String)> = COMMANDS
        .iter()
        .filter(|(c, _)| c[1..].starts_with(&p))
        .map(|(c, d)| ((*c).to_string(), (*d).to_string()))
        .collect();
    for s in skills {
        if COMMANDS.iter().any(|(c, _)| c[1..] == *s) {
            continue;
        }
        if s.to_lowercase().starts_with(&p) {
            out.push((format!("/{s}"), "skill".to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_a_message() {
        let mut s = CodeState::default();
        assert_eq!(run_command("hello", &mut s), None);
        assert_eq!(run_command("//escaped", &mut s), Some(Effect::Done));
    }

    #[test]
    fn commands_open_sheets_or_send() {
        let mut s = CodeState::default();
        run_command("/model", &mut s);
        assert_eq!(s.sheet, Some(Sheet::Model));
        run_command("/mode plan", &mut s);
        assert_eq!(s.mode, Mode::Plan);
        run_command("/effort default", &mut s);
        assert_eq!(s.effort, "default");
        s.model = "claude/opus".into();
        run_command("/ultracode on", &mut s);
        assert!(s.ultracode);
        run_command("/effort ultracode off", &mut s);
        assert!(!s.ultracode);
        s.model = "codex/gpt-6.1-sol".into();
        run_command("/ultracode", &mut s);
        assert!(s.ultracode);
        s.model = "agy/gemini-4-neon".into();
        run_command("/ultracode off", &mut s);
        assert!(!s.ultracode);
        run_command("/ultracode on", &mut s);
        assert!(s.ultracode);
        run_command("/submodel claude/haiku low", &mut s);
        assert_eq!((s.sub_model.as_deref(), s.sub_effort.as_deref()), (Some("haiku"), Some("low")));
        run_command("/subagent-model main", &mut s);
        assert_eq!(s.sub_model, None);
        run_command("/btw why?", &mut s);
        assert_eq!(s.sheet, Some(Sheet::Btw));
        assert_eq!(s.btw[0].question, "why?");
    }

    #[test]
    fn compact_waits_for_a_running_turn() {
        let mut s = CodeState::default();
        s.busy = true;
        run_command("/compact", &mut s);
        assert!(s.outbox.is_empty());
        assert!(matches!(s.items.last(), Some(Item::Notice { .. })));
    }

    #[test]
    fn copy_and_unknown() {
        let mut s = CodeState::default();
        assert_eq!(run_command("/copy", &mut s), Some(Effect::Done));
        s.apply(Ev::Text { id: "a".into(), text: "reply".into() });
        assert_eq!(run_command("/copy", &mut s), Some(Effect::Copy("reply".into())));
        run_command("/nope", &mut s);
        assert!(matches!(s.items.last(), Some(Item::Notice { text, .. }) if text.contains("/nope")));
        assert_eq!(run_command("/sidebar off", &mut s), Some(Effect::Sidebar(Some(false))));
    }

    #[test]
    fn skills_lose_to_builtins() {
        let skills = vec!["deploy".to_string(), "model".to_string()];
        assert_eq!(skill_call("/deploy now", &skills), Some(("deploy".into(), "now".into())));
        assert_eq!(skill_call("/model", &skills), None);
        assert_eq!(skill_call("//deploy", &skills), None);
        assert_eq!(skill_call("hello", &skills), None);
    }

    #[test]
    fn popup_matches_prefixes() {
        assert!(matching("/co").iter().any(|(c, _)| *c == "/compact"));
        assert!(matching("/co").iter().all(|(c, _)| c.starts_with("/co")));
        assert_eq!(matching("/").len(), COMMANDS.len());

        let skills = vec!["deploy".to_string(), "model".to_string(), "debug-ui".to_string()];
        let all = matching_with_skills("/", &skills);
        assert_eq!(all.len(), COMMANDS.len() + 2);
        assert!(all.iter().any(|(c, d)| c == "/deploy" && d == "skill"));
        assert_eq!(matching_with_skills("/de", &skills), vec![("/deploy".into(), "skill".into()), ("/debug-ui".into(), "skill".into())]);
    }
}
