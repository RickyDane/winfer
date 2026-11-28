//! Home's drill-in navigation (which chat a task card opens) and the task / machine references
//! Home's chats turn into links. Pure, so it's tested without a window.

use super::home::TaskMode;
use crate::code::state::Item;

/// What a task's chat in Home shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    /// A forked task's orchestrator (a Home session).
    Orchestrator,
    /// An agent pane here: a light task's agent, a fork's agent, or the tile of a remote
    /// task's orchestrator.
    Pane(String),
    /// A remote task's orchestrator with no tile here yet: (peer, its pane there).
    Remote(String, String),
    /// Nothing to show: its panes are gone.
    Nothing,
}

/// The chat a task opens: the agent picked before if it's still open, else the task's main one.
/// `panes`: its agent panes here; `open`: whether one still is; `tile`: the local tile of a
/// remote task's orchestrator; `remote`: that orchestrator (peer, pane).
pub fn who(mode: TaskMode, picked: Option<&str>, panes: &[String], open: impl Fn(&str) -> bool, tile: Option<String>, remote: Option<(String, String)>) -> Who {
    if let Some(p) = picked.filter(|p| panes.iter().any(|x| x == p) && open(p)) {
        return Who::Pane(p.to_string());
    }
    match mode {
        TaskMode::Fork => Who::Orchestrator,
        TaskMode::Light => panes.iter().find(|p| open(p)).map_or(Who::Nothing, |p| Who::Pane(p.clone())),
        TaskMode::Remote => match (tile, remote) {
            (Some(t), _) => Who::Pane(t),
            (None, Some((peer, pane))) => Who::Remote(peer, pane),
            _ => Who::Nothing,
        },
    }
}

/// Home's replies after the first `seen` items: the "n new" on the way back.
pub fn new_replies(items: &[Item], seen: usize) -> usize {
    items.iter().skip(seen).filter(|i| matches!(i, Item::Text { text, .. } if !text.trim().is_empty())).count()
}

/// Home's latest reply as one short line, for the strip above a task's chat.
pub fn latest_reply(items: &[Item]) -> Option<String> {
    let text = items.iter().rev().find_map(|i| match i {
        Item::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
        _ => None,
    })?;
    // The reply's lines (list items too) read as one.
    let line = text.lines().map(|l| l.trim().trim_start_matches(['#', '-', '*', '>', ' '])).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    let plain: String = line.replace("**", "").replace('`', "");
    // Links read as their text.
    let plain = strip_links(&plain);
    Some(if plain.chars().count() > 160 { format!("{}…", plain.chars().take(159).collect::<String>().trim_end()) } else { plain })
}

fn strip_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('[') {
        let Some(mid) = rest[start..].find("](").map(|m| start + m) else { break };
        let Some(end) = rest[mid..].find(')').map(|e| mid + e) else { break };
        out.push_str(&rest[..start]);
        out.push_str(&rest[start + 1..mid]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// What Home's chats link: the numbers of its tasks and the names of the paired machines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub tasks: Vec<u32>,
    pub machines: Vec<String>,
}

impl Refs {
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.machines.is_empty()
    }
}

/// The inline image a machine chip starts with (see `machine_icon`).
pub const MACHINE_ICON: &str = "winfer-icon:server";

/// Turns "Task 3" into a link that drills into it and a paired machine's name into a chip
/// (server icon + name, a link). Code, existing links and their targets stay as they are.
pub fn link_refs(text: &str, refs: &Refs) -> String {
    if refs.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 32);
    let mut fenced = false;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced || line.starts_with("    ") || line.starts_with('\t') {
            out.push_str(line);
            continue;
        }
        link_line(line, refs, &mut out);
    }
    out
}

fn link_line(line: &str, refs: &Refs, out: &mut String) {
    let b = line.as_bytes();
    let mut i = 0;
    let mut plain = 0; // start of text not yet copied
    while i < b.len() {
        match b[i] {
            // Inline code: skip to its closing run of backticks.
            b'`' => {
                let run = b[i..].iter().take_while(|c| **c == b'`').count();
                let close = "`".repeat(run);
                i = line[i + run..].find(&close).map_or(b.len(), |e| i + run + e + run);
            }
            // A link or image already: skip its text and target.
            b'[' => {
                i = skip_link(line, i).unwrap_or(i + 1);
            }
            // A bare URL or autolink: leave it alone.
            b'<' if line[i..].starts_with("<http") => {
                i = line[i..].find('>').map_or(b.len(), |e| i + e + 1);
            }
            _ if line[i..].starts_with("http://") || line[i..].starts_with("https://") || line[i..].starts_with("winfer://") => {
                i += line[i..].find(char::is_whitespace).unwrap_or(line.len() - i);
            }
            _ if boundary_before(line, i) => {
                if let Some((len, n)) = task_at(&line[i..]).filter(|(_, n)| refs.tasks.contains(n)) {
                    out.push_str(&line[plain..i]);
                    out.push_str(&format!("[{}](winfer://task/{n})", &line[i..i + len]));
                    i += len;
                    plain = i;
                    continue;
                }
                if let Some(m) = refs.machines.iter().filter(|m| !m.is_empty() && line[i..].starts_with(m.as_str()) && boundary_after(line, i + m.len())).max_by_key(|m| m.len()) {
                    out.push_str(&line[plain..i]);
                    out.push_str(&format!("[![]({MACHINE_ICON}) {m}](winfer://machine/{})", encode(m)));
                    i += m.len();
                    plain = i;
                    continue;
                }
                i += line[i..].chars().next().map_or(1, char::len_utf8);
            }
            _ => i += line[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out.push_str(&line[plain.min(line.len())..]);
}

/// The end of a `[text](target)` (or `![alt](src)`) starting at `at`, nested brackets allowed.
fn skip_link(line: &str, at: usize) -> Option<usize> {
    let b = line.as_bytes();
    let mut depth = 0;
    let mut i = at;
    while i < b.len() {
        match b[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    // `[text](target)`; a bare `[text]` is just text in brackets: only skip the bracket.
    if b.get(i + 1) == Some(&b'(') {
        return line[i + 1..].find(')').map(|e| i + 1 + e + 1);
    }
    None
}

/// "Task 12" at the start of `s`: (its length, 12).
fn task_at(s: &str) -> Option<(usize, u32)> {
    let rest = s.strip_prefix("Task ")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 4 {
        return None;
    }
    let len = 5 + digits;
    if !boundary_after(s, len) {
        return None;
    }
    Some((len, rest[..digits].parse().ok()?))
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.')
}

fn boundary_before(s: &str, at: usize) -> bool {
    s[..at].chars().next_back().is_none_or(|c| !word(c) || c == '.')
}

fn boundary_after(s: &str, at: usize) -> bool {
    let mut next = s[at..].chars();
    match next.next() {
        None => true,
        // "Task 3." ends a sentence; "mac.local" goes on.
        Some('.') => next.next().is_none_or(|c| !word(c)),
        Some(c) => !word(c),
    }
}

fn encode(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c.to_string() } else { format!("%{:02X}", c as u32) }).collect()
}

/// The server icon of a machine chip, as an SVG in `color` (an `#rrggbb`), sized to the text.
pub fn machine_icon(color: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="7" rx="2"/><rect x="3" y="14" width="18" height="7" rx="2"/><path d="M7 6.5h.01M7 17.5h.01"/></svg>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs() -> Refs {
        Refs { tasks: vec![1, 2, 12], machines: vec!["mac-studio".into(), "hetzner-01".into(), "mac".into()] }
    }

    #[test]
    fn links_task_numbers_that_exist() {
        let out = link_refs("Task 2 is ready; Task 7 is gone, Task 12.", &refs());
        assert_eq!(out, "[Task 2](winfer://task/2) is ready; Task 7 is gone, [Task 12](winfer://task/12).");
    }

    #[test]
    fn leaves_links_code_and_urls_alone() {
        let r = refs();
        assert_eq!(link_refs("[Task 2](winfer://task/2)", &r), "[Task 2](winfer://task/2)");
        assert_eq!(link_refs("see `Task 2` and ``mac-studio``", &r), "see `Task 2` and ``mac-studio``");
        assert_eq!(link_refs("```\nTask 1 on mac-studio\n```\nTask 1", &r), "```\nTask 1 on mac-studio\n```\n[Task 1](winfer://task/1)");
        assert_eq!(link_refs("https://x.dev/Task 1", &r), "https://x.dev/Task 1");
    }

    #[test]
    fn needs_word_boundaries() {
        let r = refs();
        assert_eq!(link_refs("SubTask 1 and Task 12a", &r), "SubTask 1 and Task 12a");
        assert_eq!(link_refs("my-mac-studio and mac-studio2", &r), "my-mac-studio and mac-studio2");
    }

    #[test]
    fn machines_become_chips_longest_name_first() {
        let out = link_refs("Started on mac-studio, and mac.", &refs());
        assert_eq!(out, format!("Started on [![]({MACHINE_ICON}) mac-studio](winfer://machine/mac-studio), and [![]({MACHINE_ICON}) mac](winfer://machine/mac)."));
    }

    #[test]
    fn no_refs_no_change() {
        assert_eq!(link_refs("Task 1 on mac", &Refs::default()), "Task 1 on mac");
    }

    fn panes() -> Vec<String> {
        vec!["p1".into(), "p2".into()]
    }

    #[test]
    fn fork_opens_its_orchestrator_or_the_picked_agent() {
        let open = |_: &str| true;
        assert_eq!(who(TaskMode::Fork, None, &panes(), open, None, None), Who::Orchestrator);
        assert_eq!(who(TaskMode::Fork, Some("p2"), &panes(), open, None, None), Who::Pane("p2".into()));
        // An agent that closed (or isn't the task's): back to the orchestrator.
        assert_eq!(who(TaskMode::Fork, Some("p2"), &panes(), |p| p != "p2", None, None), Who::Orchestrator);
        assert_eq!(who(TaskMode::Fork, Some("zz"), &panes(), open, None, None), Who::Orchestrator);
    }

    #[test]
    fn light_opens_its_first_open_agent() {
        assert_eq!(who(TaskMode::Light, None, &panes(), |p| p == "p2", None, None), Who::Pane("p2".into()));
        assert_eq!(who(TaskMode::Light, None, &panes(), |_| false, None, None), Who::Nothing);
    }

    #[test]
    fn remote_opens_its_tile_or_asks_for_one() {
        let r = Some(("peer".to_string(), "pane9".to_string()));
        assert_eq!(who(TaskMode::Remote, None, &[], |_| true, Some("tile".into()), r.clone()), Who::Pane("tile".into()));
        assert_eq!(who(TaskMode::Remote, None, &[], |_| true, None, r), Who::Remote("peer".into(), "pane9".into()));
        assert_eq!(who(TaskMode::Remote, None, &[], |_| true, None, None), Who::Nothing);
    }

    fn text(t: &str) -> Item {
        Item::Text { key: t.into(), text: t.into(), streaming: false }
    }

    #[test]
    fn counts_and_quotes_homes_replies() {
        let items = vec![text("old"), Item::User { key: "u".into(), text: "hi".into(), at: 0, images: vec![] }, text("## **Task 3** is [ready](winfer://task/3)\nmore"), text("  ")];
        assert_eq!(new_replies(&items, 1), 1);
        assert_eq!(new_replies(&items, 0), 2);
        assert_eq!(latest_reply(&items).as_deref(), Some("Task 3 is ready more"));
        assert_eq!(latest_reply(&[]), None);
    }
}
