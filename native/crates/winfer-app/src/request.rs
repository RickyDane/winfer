//! What an agent waiting for input is asking, read from the bottom of its screen (port of
//! `src/features/status/request.ts`): a permission prompt or an open question.

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RequestOption {
    /// The number that picks it (typing it into the pane selects the option).
    pub n: usize,
    pub label: String,
    /// The line under the option, when the TUI shows one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentRequest {
    /// "approval" or "question".
    pub kind: &'static str,
    /// The question as the agent shows it, condensed to a few lines.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<RequestOption>>,
}

// Permission prompts across CLIs: Claude ("Do you want to proceed? ❯ 1. Yes"), Codex
// ("Allow command? … Yes (y)"), and generic y/n prompts.
static APPROVAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)\b(do you want to|allow|approve|permission|proceed\?|run this command|make this edit)|[(\[]y/n[)\]]|^\s*❯?\s*1\.\s*yes\b|\byes \(y\)").unwrap()
});
/// Box-drawing, prompt markers and other chrome around TUI dialogs.
static CHROME: LazyLock<Regex> = LazyLock::new(|| Regex::new("[─━═│┃╭╮╰╯┌┐└┘├┤┬┴┼▌▐█░]").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[❯>›▶]\s*)?(?:[☐☑✓✔◯◉●○]\s*)?(\d{1,2})[.)]\s+(.+)$").unwrap());
/// Key hints under a picker ("Enter to select · ↑/↓ to navigate · Esc to cancel").
static FOOTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)enter to (select|confirm|submit)|esc to (cancel|go back)|↑/↓|tab to |ctrl\+").unwrap());
static ENDS_QUESTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\?\s*$").unwrap());
static QUESTION_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\?\s*\(").unwrap());
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+\.|❯|>)\s").unwrap());
static PERMISSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)permission|approv").unwrap());

fn clean(line: &str) -> String {
    SPACES.replace_all(&CHROME.replace_all(line, " "), " ").trim().to_string()
}

fn chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The last run of numbered options (1, 2, 3…) in `lines`, with the index of its first line.
pub fn read_options(lines: &[String]) -> Option<(Vec<RequestOption>, usize)> {
    let mut best = None;
    let mut i = 0;
    while i < lines.len() {
        let starts = OPTION.captures(&lines[i]).is_some_and(|m| &m[1] == "1");
        if !starts {
            i += 1;
            continue;
        }
        let mut options: Vec<RequestOption> = Vec::new();
        let mut j = i;
        while j < lines.len() {
            if let Some(o) = OPTION.captures(&lines[j]).filter(|o| o[1].parse::<usize>().ok() == Some(options.len() + 1)) {
                options.push(RequestOption { n: options.len() + 1, label: o[2].trim().to_string(), detail: None });
                j += 1;
                continue;
            }
            // One description line per option; anything else ends the list.
            let line = &lines[j];
            if let Some(last) = options.last_mut().filter(|l| l.detail.is_none())
                && line.chars().count() > 2
                && !FOOTER.is_match(line)
                && !ENDS_QUESTION.is_match(line)
            {
                last.detail = Some(line.clone());
                j += 1;
                continue;
            }
            break;
        }
        if options.len() >= 2 {
            best = Some((options, i));
        }
        i = j.max(i + 1);
    }
    best
}

pub fn describe_request(screen: &str, hook_message: Option<&str>) -> AgentRequest {
    let all: Vec<String> = screen.split('\n').map(clean).filter(|l| !l.is_empty()).collect();
    let lines = all[all.len().saturating_sub(30)..].to_vec();
    let tail = lines.join("\n");
    let approval = APPROVAL.is_match(&tail) || hook_message.is_some_and(|m| PERMISSION.is_match(m));
    let kind = if approval { "approval" } else { "question" };

    // The question is the last line ending in "?"; the few lines above it say what it's about.
    let picker = read_options(&lines);
    let before = picker.as_ref().map_or(lines.len(), |(_, start)| *start);
    let mut q = lines[..before].iter().rposition(|l| ENDS_QUESTION.is_match(l) || QUESTION_PAREN.is_match(l));
    // A picker's prompt needn't end in "?" ("Pick a library"): take the line above the options.
    if q.is_none() && let Some((_, start)) = picker.as_ref().filter(|(_, s)| *s > 0) {
        q = Some(start - 1);
    }
    let mut text = match q {
        Some(q) => {
            let mut parts: Vec<String> = lines[q.saturating_sub(3)..q].iter().filter(|l| !LIST_ITEM.is_match(l)).cloned().collect();
            parts.push(lines[q].clone());
            parts.join("\n")
        }
        None => hook_message.map(str::to_string).unwrap_or_else(|| lines[lines.len().saturating_sub(3)..].join("\n")),
    };
    if let Some(m) = hook_message
        && q.is_none()
        && !text.contains(m)
    {
        text = m.to_string();
    }
    if text.chars().count() > 280 {
        text = format!("{}…", chars(&text, 277));
    }
    let options = picker.map(|(options, _)| {
        options.into_iter().map(|o| RequestOption { n: o.n, label: chars(&o.label, 120), detail: o.detail.map(|d| chars(&d, 160)) }).collect()
    });
    AgentRequest { kind, text, options }
}

#[cfg(test)]
mod tests {
    use super::describe_request;

    #[test]
    fn claude_permission_prompts_are_approvals_with_options() {
        let screen = "╭──────╮\n│ Bash command │\n│ rm -rf build │\n│ Do you want to proceed? │\n│ ❯ 1. Yes │\n│ 2. No, and tell Claude │\n╰──────╯";
        let r = describe_request(screen, None);
        assert_eq!(r.kind, "approval");
        assert_eq!(r.text, "Bash command\nrm -rf build\nDo you want to proceed?");
        let options = r.options.unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].label, "No, and tell Claude");
    }

    #[test]
    fn open_questions_use_the_last_question_line() {
        let r = describe_request("I looked at the code.\nWhich database should I use?\n", None);
        assert_eq!(r.kind, "question");
        assert_eq!(r.text, "I looked at the code.\nWhich database should I use?");
        assert_eq!(r.options, None);
    }

    #[test]
    fn pickers_without_a_question_mark_take_the_line_above() {
        let r = describe_request("Pick a library\n1. serde\nFast and common\n2. miniserde\nEnter to select", None);
        assert_eq!(r.text, "Pick a library");
        assert_eq!(r.options.unwrap()[0].detail.as_deref(), Some("Fast and common"));
    }

    #[test]
    fn hook_messages_stand_in_without_a_question() {
        let r = describe_request("working...\ndone", Some("Claude needs your permission to use Bash"));
        assert_eq!(r.kind, "approval");
        assert_eq!(r.text, "Claude needs your permission to use Bash");
    }
}
