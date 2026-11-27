//! `@workspace` mentions in Home's chat: the composer offers the workspaces of every machine
//! after `@`, and a message that mentions some goes to the model with a note saying exactly
//! which workspace each one is, so the orchestrator needn't list or guess them.

/// A workspace the user can mention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// What follows the `@`: "api", or "mac-mini/web" for another machine's workspace.
    pub token: String,
    /// The popup's second column: "this machine · 2 agents".
    pub detail: String,
    /// What the model is told it stands for.
    pub note: String,
}

/// A name as a mention token: lowercase, spaces and other separators as `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() || matches!(ch, '_' | '.') {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The known mentions in `text`, in the order they first appear. `@token` counts when it starts
/// the text or follows whitespace or an opening bracket, and ends the text or is followed by
/// whitespace or punctuation.
pub fn mentioned<'a>(text: &str, known: &'a [Mention]) -> Vec<&'a Mention> {
    let lower = text.to_lowercase();
    let mut found: Vec<(usize, &Mention)> = Vec::new();
    // Longest first, so "@mac-mini/web" isn't taken for a shorter "@mac-mini".
    let mut by_len: Vec<&Mention> = known.iter().collect();
    by_len.sort_by_key(|m| std::cmp::Reverse(m.token.len()));
    let mut taken: Vec<std::ops::Range<usize>> = Vec::new();
    for m in by_len {
        let needle = format!("@{}", m.token.to_lowercase());
        let mut from = 0;
        while let Some(i) = lower[from..].find(&needle).map(|i| i + from) {
            let end = i + needle.len();
            let before_ok = lower[..i].chars().next_back().is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '[' | '"' | '\''));
            let after_ok = lower[end..].chars().next().is_none_or(|c| c.is_whitespace() || matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '"' | '\''));
            if before_ok && after_ok && !taken.iter().any(|r| r.start < end && i < r.end) {
                taken.push(i..end);
                if !found.iter().any(|(_, f)| f.token == m.token) {
                    found.push((i, m));
                }
            }
            from = end;
        }
    }
    found.sort_by_key(|(i, _)| *i);
    found.into_iter().map(|(_, m)| m).collect()
}

/// The note for a message's mentions, or None when it mentions none.
pub fn note_for(text: &str, known: &[Mention]) -> Option<String> {
    let hits = mentioned(text, known);
    if hits.is_empty() {
        return None;
    }
    let lines: Vec<String> = hits.iter().map(|m| format!("- @{} = {}", m.token, m.note)).collect();
    // Starts like `MENTION_NOTE` (after the blank line), so restored transcripts leave it out.
    Some(format!("{} (no need to look them up):\n{}", crate::code::state::MENTION_NOTE.trim_start(), lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(token: &str) -> Mention {
        Mention { token: token.into(), detail: String::new(), note: format!("workspace {token}") }
    }

    #[test]
    fn names_become_tokens() {
        assert_eq!(slug("My App"), "my-app");
        assert_eq!(slug("Mac Studio von Ricky"), "mac-studio-von-ricky");
        assert_eq!(slug(" winfer.app_2 "), "winfer.app_2");
        assert_eq!(slug("a / b"), "a-b");
    }

    #[test]
    fn mentions_are_found_whole() {
        let known = [m("api"), m("web"), m("mac-mini/web"), m("api-v2")];
        let tokens = |t: &str| mentioned(t, &known).iter().map(|m| m.token.clone()).collect::<Vec<_>>();
        assert_eq!(tokens("fix the login in @api, then @mac-mini/web."), ["api", "mac-mini/web"]);
        // Not part of a longer token, an email or a path.
        assert_eq!(tokens("@api-v2 and me@api and x/@web"), ["api-v2"]);
        // Case doesn't matter; repeats count once; order of first appearance.
        assert_eq!(tokens("(@WEB) vs @api vs @web"), ["web", "api"]);
        assert!(tokens("no mentions here").is_empty());
    }

    #[test]
    fn the_note_lists_each_mention() {
        let known = [m("api")];
        assert_eq!(note_for("hi", &known), None);
        let note = note_for("look at @api", &known).unwrap();
        assert!(note.ends_with("- @api = workspace api"), "{note}");
    }
}
