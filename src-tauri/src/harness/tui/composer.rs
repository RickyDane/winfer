//! The prompt editor: multi-line text with a cursor, history, collapsed pastes and images.

use std::path::PathBuf;

use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub struct Composer {
    pub text: String,
    /// Byte offset of the cursor.
    pub cursor: usize,
    pastes: Vec<String>,
    pub images: Vec<PathBuf>,
    history_pos: Option<usize>,
    draft: String,
}

const IMAGE_EXT: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Paths emitted by terminals on drop: quoted Windows paths or shell-escaped Unix paths.
/// Backslashes are otherwise literal (in particular in Windows drive/UNC paths).
pub fn path_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && (word.is_empty() || word == "@") && (c == '\'' || c == '"') {
            quote = Some(c);
        } else if c == '\\' && chars.peek().is_some_and(|n| {
            (quote.is_none() && (n.is_whitespace() || *n == '\'' || *n == '"')) || (quote == Some('"') && *n == '"')
        }) {
            word.push(chars.next().unwrap());
        } else if quote.is_none() && c.is_whitespace() {
            if !word.is_empty() { words.push(std::mem::take(&mut word)); }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() { words.push(word); }
    words
}

impl Composer {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        *self = Composer::default();
    }

    /// True while Up/Down is stepping through history (until the text is edited).
    pub fn browsing_history(&self) -> bool {
        self.history_pos.is_some()
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.history_pos = None;
    }

    /// Long text collapses; dropped file paths become image attachments or quoted mentions.
    pub fn paste(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let whole = PathBuf::from(s.trim());
        let paths: Vec<PathBuf> = if whole.is_file() { vec![whole] } else { path_words(&s).into_iter().map(PathBuf::from).collect() };
        if !paths.is_empty() && paths.iter().all(|p| p.is_file()) {
            for path in paths {
                let is_image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()));
                if is_image {
                    self.attach_image(path);
                } else {
                    self.insert(&format!("@\"{}\" ", path.to_string_lossy().replace('"', "\\\"")));
                }
            }
            return;
        }
        let lines = s.lines().count();
        if lines > 4 || s.len() > 1200 {
            self.pastes.push(s.clone());
            let tag = format!("[Pasted text #{} +{lines} lines]", self.pastes.len());
            self.insert(&tag);
        } else {
            self.insert(&s);
        }
    }

    /// Attaches an image file, shown as an `[Image #n]` placeholder at the cursor.
    pub fn attach_image(&mut self, path: PathBuf) {
        self.images.push(path);
        let tag = format!("[Image #{}] ", self.images.len());
        self.insert(&tag);
    }

    /// The text to send (pastes expanded), and attached images; clears the editor.
    pub fn take(&mut self) -> (String, String, Vec<PathBuf>) {
        let shown = self.text.clone();
        let mut text = self.text.clone();
        for (i, p) in self.pastes.iter().enumerate() {
            let lines = p.lines().count();
            text = text.replace(&format!("[Pasted text #{} +{lines} lines]", i + 1), p);
        }
        let images = std::mem::take(&mut self.images).into_iter().enumerate()
            .filter(|(i, _)| self.text.contains(&format!("[Image #{}]", i + 1)))
            .map(|(_, path)| path).collect();
        self.clear();
        (text, shown, images)
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.cursor].char_indices().last().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..].chars().next().map(|c| self.cursor + c.len_utf8()).unwrap_or(self.cursor)
    }

    pub fn left(&mut self) {
        self.cursor = self.prev_boundary();
    }

    pub fn right(&mut self) {
        self.cursor = self.next_boundary();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.history_pos = None;
        // A placeholder goes away whole.
        if self.text[..self.cursor].ends_with(']') {
            if let Some(start) = self.text[..self.cursor].rfind('[') {
                let tag = &self.text[start..self.cursor];
                if tag.starts_with("[Pasted text #") || tag.starts_with("[Image #") {
                    self.text.replace_range(start..self.cursor, "");
                    self.cursor = start;
                    return;
                }
            }
        }
        let p = self.prev_boundary();
        self.text.replace_range(p..self.cursor, "");
        self.cursor = p;
    }

    pub fn delete(&mut self) {
        let n = self.next_boundary();
        if n > self.cursor {
            self.history_pos = None;
        }
        self.text.replace_range(self.cursor..n, "");
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    pub fn kill_to_start(&mut self) {
        let s = self.line_start();
        let s = if s == self.cursor && s > 0 { s - 1 } else { s };
        self.text.replace_range(s..self.cursor, "");
        self.cursor = s;
    }

    pub fn kill_to_end(&mut self) {
        let e = self.line_end();
        let e = if e == self.cursor && e < self.text.len() { e + 1 } else { e };
        self.text.replace_range(self.cursor..e, "");
    }

    pub fn word_left(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        self.cursor = trimmed.rfind(|c: char| c.is_whitespace() || "/._-".contains(c)).map(|i| i + 1).unwrap_or(0);
        if self.cursor == before.len() && self.cursor > 0 {
            self.left();
        }
    }

    pub fn word_right(&mut self) {
        let after = &self.text[self.cursor..];
        let skip = after.len() - after.trim_start().len();
        let rest = &after[skip..];
        let end = rest.find(|c: char| c.is_whitespace() || "/._-".contains(c)).map(|i| i.max(1)).unwrap_or(rest.len());
        self.cursor += skip + end;
    }

    pub fn delete_word(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.text.replace_range(self.cursor..end, "");
    }

    /// Moves up a line; false when already on the first (the caller goes to history).
    pub fn up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let col = self.text[start..self.cursor].chars().count();
        let prev_start = self.text[..start - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prev = &self.text[prev_start..start - 1];
        self.cursor = prev_start + prev.char_indices().nth(col).map(|(i, _)| i).unwrap_or(prev.len());
        true
    }

    pub fn down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let col = self.text[self.line_start()..self.cursor].chars().count();
        let next_start = end + 1;
        let next_end = self.text[next_start..].find('\n').map(|i| next_start + i).unwrap_or(self.text.len());
        let next = &self.text[next_start..next_end];
        self.cursor = next_start + next.char_indices().nth(col).map(|(i, _)| i).unwrap_or(next.len());
        true
    }

    pub fn history_prev(&mut self, history: &[String]) {
        if history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => {
                self.draft = self.text.clone();
                history.len() - 1
            }
            Some(0) => return,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.text = history[pos].clone();
        self.cursor = self.text.len();
    }

    pub fn history_next(&mut self, history: &[String]) {
        let Some(pos) = self.history_pos else { return };
        if pos + 1 >= history.len() {
            self.history_pos = None;
            self.text = std::mem::take(&mut self.draft);
        } else {
            self.history_pos = Some(pos + 1);
            self.text = history[pos + 1].clone();
        }
        self.cursor = self.text.len();
    }

    /// The word being typed at the cursor (for completions): (start byte, word).
    pub fn current_word(&self) -> (usize, &str) {
        let before = &self.text[..self.cursor];
        let start = before.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        (start, &self.text[start..self.cursor])
    }

    pub fn replace_word(&mut self, start: usize, with: &str) {
        self.text.replace_range(start..self.cursor, with);
        self.cursor = start + with.len();
    }

    /// Visual rows when wrapped at `width` (character wrap), and the cursor's (row, col).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = vec![String::new()];
        let mut col = 0;
        let mut cursor = (0, 0);
        for (i, c) in self.text.char_indices() {
            if i == self.cursor {
                cursor = (rows.len() - 1, col);
            }
            if c == '\n' {
                rows.push(String::new());
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col + w > width {
                rows.push(String::new());
                col = 0;
            }
            rows.last_mut().unwrap().push(c);
            col += w;
        }
        if self.cursor >= self.text.len() {
            if col >= width {
                rows.push(String::new());
                col = 0;
            }
            cursor = (rows.len() - 1, col);
        }
        (rows, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_terminal_drop_paths_without_losing_windows_separators() {
        assert_eq!(path_words(r#""C:\My Files\a.png" 'C:\My Files\b.txt'"#), vec![r"C:\My Files\a.png", r"C:\My Files\b.txt"]);
        assert_eq!(path_words(r"/tmp/my\ image.png /tmp/other.txt"), vec!["/tmp/my image.png", "/tmp/other.txt"]);
        assert_eq!(path_words(r#"see @"/tmp/my notes.txt""#), vec!["see", "@/tmp/my notes.txt"]);
        assert_eq!(path_words("don't skip @file.txt"), vec!["don't", "skip", "@file.txt"]);
    }

    #[test]
    fn mixed_file_drops_attach_images_and_mention_other_files() {
        let dir = std::env::temp_dir().join(format!("winfer-drop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("my image.PNG");
        let file = dir.join("my notes.txt");
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(&file, b"notes").unwrap();
        let mut c = Composer::default();
        c.paste(&format!("\"{}\" \"{}\"", image.display(), file.display()));
        let (text, _, images) = c.take();
        assert_eq!(images, vec![image.clone()]);
        assert!(text.contains(&format!("@\"{}\"", file.display())));
        std::fs::remove_file(image).unwrap();
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn ordinary_pastes_are_not_rewritten_as_paths() {
        let mut c = Composer::default();
        c.paste("please review 'my notes' and C:\\missing.txt");
        assert_eq!(c.text, "please review 'my notes' and C:\\missing.txt");
    }

    #[test]
    fn removed_image_placeholders_are_not_sent() {
        let mut c = Composer::default();
        c.attach_image(PathBuf::from("removed.png"));
        c.backspace(); // trailing space
        c.backspace(); // image placeholder
        let (_, _, images) = c.take();
        assert!(images.is_empty());
    }

    #[test]
    fn edits_and_moves() {
        let mut c = Composer::default();
        c.insert("hello world");
        c.word_left();
        assert_eq!(c.cursor, 6);
        c.delete_word();
        assert_eq!(c.text, "world");
        c.end();
        c.insert("\nnext");
        assert!(c.up());
        assert!(!c.up());
        assert!(c.down());
    }

    #[test]
    fn long_pastes_collapse_and_expand() {
        let mut c = Composer::default();
        c.insert("see ");
        c.paste("1\n2\n3\n4\n5\n6");
        assert_eq!(c.text, "see [Pasted text #1 +6 lines]");
        let (text, _, _) = c.take();
        assert_eq!(text, "see 1\n2\n3\n4\n5\n6");
    }

    #[test]
    fn image_paths_become_attachments() {
        let path = std::env::temp_dir().join(format!("winfer-composer-{}.png", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        let mut c = Composer::default();
        c.paste(&format!("'{}'", path.display()));
        assert_eq!(c.text, "[Image #1] ");
        let (_, _, images) = c.take();
        assert_eq!(images, vec![path.clone()]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn backspace_removes_a_placeholder() {
        let mut c = Composer::default();
        c.paste(&"x\n".repeat(10));
        c.backspace();
        assert!(c.text.is_empty());
    }

    #[test]
    fn layout_wraps_and_places_the_cursor() {
        let mut c = Composer::default();
        c.insert("abcdef\ngh");
        let (rows, cur) = c.layout(4);
        assert_eq!(rows, vec!["abcd", "ef", "gh"]);
        assert_eq!(cur, (2, 2));
    }

    #[test]
    fn history_round_trip() {
        let h = vec!["a".to_string(), "b".to_string()];
        let mut c = Composer::default();
        c.insert("draft");
        c.history_prev(&h);
        assert_eq!(c.text, "b");
        c.history_prev(&h);
        assert_eq!(c.text, "a");
        c.history_next(&h);
        c.history_next(&h);
        assert_eq!(c.text, "draft");
    }
}
