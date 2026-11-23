//! Push-to-talk dictation for the winfer code composer, on the OS's own speech recognizer:
//! SFSpeechRecognizer on macOS (`macos.swift`), Windows.Media.SpeechRecognition on Windows.
//! Elsewhere `supported()` is false and the composer shows no mic button.
//!
//! One session at a time. Hold to talk: `start` on press, `stop` on release; events arrive on the
//! channel and every session ends with exactly one `Event::Ended`.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as platform;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Everything heard so far; `done` once it is final.
    Text { text: String, done: bool },
    Error(String),
    Ended,
}

pub fn supported() -> bool {
    cfg!(any(target_os = "macos", windows))
}

/// Starts listening, ending any session that is still running.
pub fn start(tx: async_channel::Sender<Event>) {
    #[cfg(any(target_os = "macos", windows))]
    platform::start(tx);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = tx.try_send(Event::Error("Dictation isn't available on this system.".into()));
        let _ = tx.try_send(Event::Ended);
    }
}

/// Stops listening; the final text follows, then `Ended`.
pub fn stop() {
    #[cfg(any(target_os = "macos", windows))]
    platform::stop();
}

/// Drops the session without waiting for its final text.
pub fn cancel() {
    #[cfg(any(target_os = "macos", windows))]
    platform::cancel();
}

/// The draft with dictated `text` put in at the char offset `at` (where the caret was when it
/// started), with a space on either side where it would run into a word.
pub fn insert(draft: &str, at: usize, text: &str) -> (String, usize) {
    let text = text.trim();
    let at = draft.char_indices().nth(at).map_or(draft.len(), |(i, _)| i);
    let (before, after) = draft.split_at(at);
    if text.is_empty() {
        return (draft.to_string(), before.chars().count());
    }
    let mut out = String::with_capacity(draft.len() + text.len() + 2);
    out.push_str(before);
    if before.chars().last().is_some_and(|c| !c.is_whitespace()) {
        out.push(' ');
    }
    out.push_str(text);
    let caret = out.chars().count();
    if after.chars().next().is_some_and(|c| !c.is_whitespace()) {
        out.push(' ');
    }
    out.push_str(after);
    (out, caret)
}

#[cfg(test)]
mod tests {
    use super::insert;

    #[test]
    fn inserts_with_spaces_only_where_needed() {
        assert_eq!(insert("", 0, " hello world "), ("hello world".into(), 11));
        assert_eq!(insert("fix", 3, "the login bug"), ("fix the login bug".into(), 17));
        assert_eq!(insert("fix ", 4, "it"), ("fix it".into(), 6));
        assert_eq!(insert("ab", 1, "x"), ("a x b".into(), 3));
        assert_eq!(insert("ä ö", 1, "ü"), ("ä ü ö".into(), 3));
        assert_eq!(insert("keep", 99, ""), ("keep".into(), 4));
    }
}
