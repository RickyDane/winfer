//! The server's chat panes (`winferd code --serve`) as their events describe them, parsed from the
//! output the panes stream to desktops: phones read a chat's thread from it and answer what it
//! asks, as they do on a desktop (winfer_core::code::mobile).

use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;
use winfer_core::code::mobile;
use winfer_core::code::protocol::{Cmd, Decision, LineBuffer, parse_line};
use winfer_core::code::state::CodeState;
use winfer_core::request::AgentRequest;

#[derive(Default)]
struct Chat {
    lines: LineBuffer,
    state: CodeState,
    /// A form's answers so far: (its approval id, the answers).
    answers: Option<(String, Vec<String>)>,
}

static CHATS: LazyLock<Mutex<HashMap<String, Chat>>> = LazyLock::new(Default::default);

/// Output of the chat pane `pane` (JSON lines, maybe cut mid-line).
pub fn tap(pane: &str, bytes: &[u8]) {
    let mut chats = CHATS.lock();
    let chat = chats.entry(pane.to_string()).or_default();
    for line in chat.lines.push(bytes) {
        // Not JSON (a crash's text): not a message of the chat.
        if line.trim_start().starts_with('{')
            && let Some(ev) = parse_line(&line)
        {
            chat.state.apply(ev);
        }
    }
    // Nothing is sent from this copy: the process gets commands from `send`.
    chat.state.outbox.clear();
}

/// A message sent to the chat as a command (the chat doesn't echo those).
pub fn note_user(pane: &str, text: &str) {
    CHATS.lock().entry(pane.to_string()).or_default().state.apply(winfer_core::code::protocol::Ev::User { text: text.to_string() });
}

pub fn forget(pane: &str) {
    CHATS.lock().remove(pane);
}

fn answered(chat: &Chat) -> usize {
    let first = chat.state.approvals.first().map(|a| a.id.as_str());
    chat.answers.as_ref().filter(|(id, _)| Some(id.as_str()) == first).map_or(0, |(_, v)| v.len())
}

/// The chat's thread as a phone shows it; None when `pane` isn't a chat seen here.
pub fn thread(pane: &str, limit: usize) -> Option<Vec<Value>> {
    let chats = CHATS.lock();
    let chat = chats.get(pane)?;
    let mut list = mobile::live_thread(&chat.state, limit);
    let extra = list.len().saturating_sub(limit);
    list.drain(..extra);
    Some(list)
}

/// What the chat waits on (an approval or a question).
pub fn request(pane: &str) -> Option<AgentRequest> {
    let chats = CHATS.lock();
    let chat = chats.get(pane)?;
    mobile::request(&chat.state, answered(chat))
}

/// Whether the chat waits on a question (its answers are typed or picked).
pub fn asks_question(pane: &str) -> bool {
    CHATS.lock().get(pane).and_then(|c| c.state.approvals.first()).is_some_and(|a| !a.questions.is_empty())
}

/// Allows or denies the tool call the chat waits on; false when it waits on none.
pub fn decide(pane: &str, allow: bool) -> bool {
    let id = {
        let mut chats = CHATS.lock();
        let Some(chat) = chats.get_mut(pane) else { return false };
        let Some(id) = chat.state.approvals.first().filter(|a| a.questions.is_empty()).map(|a| a.id.clone()) else { return false };
        chat.state.approvals.remove(0);
        id
    };
    crate::host::write_chat(pane, &Cmd::Approve { id, decision: if allow { Decision::Allow } else { Decision::Deny } });
    true
}

/// A phone's answer to the chat's question: option `n` (or the typed text on the row after the
/// options), or with `n` = 0 the typed text itself. The last answer of a form sends them all.
pub fn answer(pane: &str, n: usize, typed: &str) -> Result<String, String> {
    let (answer, done) = {
        let mut chats = CHATS.lock();
        let chat = chats.get_mut(pane).ok_or("That chat isn't running.")?;
        let (id, total) = chat.state.approvals.first().filter(|a| !a.questions.is_empty()).map(|a| (a.id.clone(), a.questions.len())).ok_or("It isn't asking anything now.")?;
        let answer = if n == 0 { Some(typed.trim().to_string()).filter(|t| !t.is_empty()) } else { mobile::pick(&chat.state, answered(chat), n, typed) };
        let answer = answer.ok_or("That choice is no longer on screen.")?;
        if chat.answers.as_ref().is_none_or(|(a, _)| *a != id) {
            chat.answers = Some((id.clone(), Vec::new()));
        }
        let list = &mut chat.answers.as_mut().unwrap().1;
        list.push(answer.clone());
        let done = (list.len() >= total).then(|| {
            chat.state.approvals.remove(0);
            (id, chat.answers.take().map(|(_, v)| v).unwrap_or_default())
        });
        (answer, done)
    };
    if let Some((id, answers)) = done {
        crate::host::write_chat(pane, &Cmd::Answer { id, answers });
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_chat_from_its_output() {
        let pane = "test-chat";
        tap(pane, br#"{"type":"user","text":"hi"}"#);
        tap(pane, b"\n{\"type\":\"text\",\"id\":\"a\",\"text\":\"hello\"}\n{\"type\":\"approval\",\"id\":\"x\",\"tool\":\"bash\",\"title\":\"ls\"}\n");
        let thread = thread(pane, 50).unwrap();
        assert!(thread.iter().any(|m| m["text"] == "hello"), "{thread:?}");
        assert_eq!(request(pane).map(|r| r.kind), Some("approval"));
        forget(pane);
    }
}
