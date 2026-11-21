//! Streaming extractor for small metadata fields inside a tool call's partial JSON arguments.
//!
//! Scans incoming JSON chunks in O(chunk.len()) time and O(1) memory, skipping huge string values
//! (such as `content` or `new_string` in a file write) while still capturing early or trailing
//! metadata fields (`file_path`, `path`, `command`, `cmd`, `description`, `pattern`, `url`,
//! `query`, `server`, `tool`, `args.*`, etc.) the moment they finish streaming.

use serde_json::{Map, Value};

/// Maximum byte length of a string field we keep in full. Larger payloads (file bodies, patches)
/// are skipped without buffering so memory stays bounded even for multi-megabyte tool calls.
const MAX_STR_LEN: usize = 512;

/// First-line preview threshold for long shell commands (e.g. heredocs).
const CMD_PREVIEW_LEN: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Root,
    Args,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    KeyOrEnd,
    Colon,
    Value,
    CommaOrEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrRole {
    Key { in_args: bool },
    Value { in_args: bool },
    Skip,
}

#[derive(Debug, Default)]
pub(crate) struct PartialJsonFields {
    stack: Vec<(Container, Expect)>,
    root_key: Option<String>,
    args_key: Option<String>,
    in_string: bool,
    escaped: bool,
    str_role: Option<StrRole>,
    overflow: bool,
    preview_emitted: bool,
    raw: String,
    in_scalar: bool,
    scalar_in_args: bool,
    scalar: String,
    fields: Map<String, Value>,
    args: Map<String, Value>,
}

impl PartialJsonFields {
    /// Feeds a chunk of partial JSON arguments. Returns `true` if any extracted field changed.
    pub fn feed(&mut self, chunk: &str) -> bool {
        let mut changed = false;
        for ch in chunk.chars() {
            if self.in_string {
                if self.feed_string_char(ch) {
                    changed = true;
                }
                continue;
            }
            if self.in_scalar {
                if matches!(ch, ',' | '}' | ']' | ' ' | '\t' | '\r' | '\n') {
                    if self.finish_scalar() {
                        changed = true;
                    }
                    // Fall through to process the delimiter normally.
                } else {
                    if self.scalar.len() < 32 {
                        self.scalar.push(ch);
                    }
                    continue;
                }
            }
            self.feed_structural_char(ch);
        }
        changed
    }

    /// Snapshot of all extracted fields so far as a JSON object.
    pub fn value(&self) -> Value {
        let mut out = self.fields.clone();
        if !self.args.is_empty() {
            out.insert("args".into(), Value::Object(self.args.clone()));
        }
        Value::Object(out)
    }

    fn current_value_key(&self, in_args: bool) -> Option<&str> {
        if in_args { self.args_key.as_deref() } else { self.root_key.as_deref() }
    }

    fn is_cmd_key(&self, in_args: bool) -> bool {
        matches!(self.current_value_key(in_args), Some("command" | "cmd"))
    }

    fn insert_field(&mut self, in_args: bool, val: Value) -> bool {
        let key = if in_args { self.args_key.as_ref() } else { self.root_key.as_ref() };
        let Some(k) = key.cloned() else { return false };
        let map = if in_args { &mut self.args } else { &mut self.fields };
        if map.get(&k) == Some(&val) {
            return false;
        }
        map.insert(k, val);
        true
    }

    fn emit_cmd_preview(&mut self, in_args: bool, end_idx: usize) -> bool {
        if self.preview_emitted || end_idx <= 1 {
            return false;
        }
        let mut candidate = String::with_capacity(end_idx + 1);
        candidate.push_str(&self.raw[..end_idx]);
        candidate.push('"');
        if let Ok(decoded) = serde_json::from_str::<String>(&candidate) {
            let line = decoded.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                self.preview_emitted = true;
                return self.insert_field(in_args, Value::String(line.to_string()));
            }
        }
        false
    }

    fn feed_string_char(&mut self, ch: char) -> bool {
        let role = self.str_role.unwrap_or(StrRole::Skip);
        let mut changed = false;

        if self.escaped {
            self.escaped = false;
            if !self.overflow {
                if let StrRole::Value { in_args } = role {
                    if ch == 'n' && self.is_cmd_key(in_args) && !self.preview_emitted {
                        let before_slash = self.raw.len().saturating_sub(1);
                        changed = self.emit_cmd_preview(in_args, before_slash);
                    }
                }
                if self.raw.len() + ch.len_utf8() <= MAX_STR_LEN + 1 {
                    self.raw.push(ch);
                } else {
                    self.overflow = true;
                }
            }
            return changed;
        }

        if ch == '\\' {
            self.escaped = true;
            if !self.overflow {
                if self.raw.len() + 1 <= MAX_STR_LEN + 1 {
                    self.raw.push(ch);
                } else {
                    if let StrRole::Value { in_args } = role {
                        if self.is_cmd_key(in_args) && !self.preview_emitted {
                            let end = self.raw.len();
                            changed = self.emit_cmd_preview(in_args, end);
                        }
                    }
                    self.overflow = true;
                }
            }
            return changed;
        }

        if ch == '"' {
            self.in_string = false;
            let overflow = self.overflow;
            self.overflow = false;
            self.str_role = None;
            match role {
                StrRole::Key { in_args } => {
                    let parsed = if !overflow {
                        self.raw.push('"');
                        serde_json::from_str::<String>(&self.raw).ok()
                    } else {
                        None
                    };
                    if in_args {
                        self.args_key = parsed;
                    } else {
                        self.root_key = parsed;
                    }
                    if let Some((_, exp)) = self.stack.last_mut() {
                        *exp = Expect::Colon;
                    }
                }
                StrRole::Value { in_args } => {
                    if !overflow {
                        self.raw.push('"');
                        if let Ok(decoded) = serde_json::from_str::<String>(&self.raw) {
                            changed = self.insert_field(in_args, Value::String(decoded));
                        }
                    }
                    if in_args {
                        self.args_key = None;
                    } else {
                        self.root_key = None;
                    }
                    if let Some((_, exp)) = self.stack.last_mut() {
                        *exp = Expect::CommaOrEnd;
                    }
                }
                StrRole::Skip => {}
            }
            self.raw.clear();
            self.preview_emitted = false;
            return changed;
        }

        if !self.overflow {
            if self.raw.len() + ch.len_utf8() <= MAX_STR_LEN + 1 {
                self.raw.push(ch);
                if let StrRole::Value { in_args } = role {
                    if self.is_cmd_key(in_args) && !self.preview_emitted && self.raw.len() >= CMD_PREVIEW_LEN {
                        let end = self.raw.len();
                        changed = self.emit_cmd_preview(in_args, end);
                    }
                }
            } else {
                if let StrRole::Value { in_args } = role {
                    if self.is_cmd_key(in_args) && !self.preview_emitted {
                        let end = self.raw.len();
                        changed = self.emit_cmd_preview(in_args, end);
                    }
                }
                self.overflow = true;
            }
        }
        changed
    }

    fn finish_scalar(&mut self) -> bool {
        self.in_scalar = false;
        let in_args = self.scalar_in_args;
        let val = match self.scalar.as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            s => s.parse::<i64>().ok().map(|n| Value::Number(n.into())).or_else(|| s.parse::<u64>().ok().map(|n| Value::Number(n.into()))),
        };
        self.scalar.clear();
        let changed = val.is_some_and(|v| self.insert_field(in_args, v));
        if in_args {
            self.args_key = None;
        } else {
            self.root_key = None;
        }
        if let Some((_, exp)) = self.stack.last_mut() {
            *exp = Expect::CommaOrEnd;
        }
        changed
    }

    fn feed_structural_char(&mut self, ch: char) {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {}
            '{' => {
                if self.stack.is_empty() {
                    self.stack.push((Container::Root, Expect::KeyOrEnd));
                } else {
                    let top = *self.stack.last().unwrap();
                    let is_args = top == (Container::Root, Expect::Value) && self.root_key.as_deref() == Some("args");
                    if let Some((c, exp)) = self.stack.last_mut() {
                        if matches!(c, Container::Root | Container::Args) && *exp == Expect::Value {
                            *exp = Expect::CommaOrEnd;
                            if *c == Container::Args {
                                self.args_key = None;
                            } else {
                                self.root_key = None;
                            }
                        }
                    }
                    if is_args {
                        self.args_key = None;
                        self.stack.push((Container::Args, Expect::KeyOrEnd));
                    } else {
                        self.stack.push((Container::Other, Expect::Value));
                    }
                }
            }
            '[' => {
                if let Some((c, exp)) = self.stack.last_mut() {
                    if matches!(c, Container::Root | Container::Args) && *exp == Expect::Value {
                        *exp = Expect::CommaOrEnd;
                        if *c == Container::Args {
                            self.args_key = None;
                        } else {
                            self.root_key = None;
                        }
                    }
                }
                self.stack.push((Container::Other, Expect::Value));
            }
            '}' | ']' => {
                self.stack.pop();
            }
            ':' => {
                if let Some((c, exp)) = self.stack.last_mut() {
                    if matches!(c, Container::Root | Container::Args) && *exp == Expect::Colon {
                        *exp = Expect::Value;
                    }
                }
            }
            ',' => {
                if let Some((c, exp)) = self.stack.last_mut() {
                    if matches!(c, Container::Root | Container::Args) {
                        *exp = Expect::KeyOrEnd;
                        if *c == Container::Args {
                            self.args_key = None;
                        } else {
                            self.root_key = None;
                        }
                    }
                }
            }
            '"' => {
                let role = match self.stack.last().copied() {
                    Some((Container::Root, Expect::KeyOrEnd)) => StrRole::Key { in_args: false },
                    Some((Container::Args, Expect::KeyOrEnd)) => StrRole::Key { in_args: true },
                    Some((Container::Root, Expect::Value)) => StrRole::Value { in_args: false },
                    Some((Container::Args, Expect::Value)) => StrRole::Value { in_args: true },
                    _ => StrRole::Skip,
                };
                self.in_string = true;
                self.escaped = false;
                self.overflow = false;
                self.preview_emitted = false;
                self.str_role = Some(role);
                self.raw.clear();
                self.raw.push('"');
            }
            _ => {
                if let Some((c @ (Container::Root | Container::Args), Expect::Value)) = self.stack.last().copied() {
                    self.in_scalar = true;
                    self.scalar_in_args = c == Container::Args;
                    self.scalar.clear();
                    self.scalar.push(ch);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_file_path_early_and_skips_huge_content() {
        let mut p = PartialJsonFields::default();
        assert!(!p.feed(r#"{"file_"#));
        assert!(!p.feed(r#"path": "/repo/src/"#));
        assert!(p.feed(r#"big.rs", "content": ""#));
        assert_eq!(p.value(), json!({ "file_path": "/repo/src/big.rs" }));
        // 100KB of JSON-escaped file content emits no further updates and doesn't grow memory.
        let chunk = r#"let x = \"quoted \\\"value\\\"\";\n"#.repeat(400);
        for _ in 0..10 {
            assert!(!p.feed(&chunk));
        }
        assert!(!p.feed(r#""}"#));
        assert_eq!(p.value(), json!({ "file_path": "/repo/src/big.rs" }));
    }

    #[test]
    fn extracts_file_path_even_after_huge_content() {
        let mut p = PartialJsonFields::default();
        let big = "a".repeat(20_000);
        assert!(!p.feed(&format!(r#"{{"content":"{big}","#)));
        assert!(p.feed(r#""file_path":"src/late.rs"}"#));
        assert_eq!(p.value(), json!({ "file_path": "src/late.rs" }));
    }

    #[test]
    fn handles_escapes_booleans_and_nested_mcp_args() {
        let mut p = PartialJsonFields::default();
        assert!(p.feed(r#"{"server":"winfer","tool":"visualize","run_in_background":true,"args":{"title":"Build \"v2\"","blocks":[{"a":1}]}}"#));
        assert_eq!(
            p.value(),
            json!({
                "server": "winfer",
                "tool": "visualize",
                "run_in_background": true,
                "args": { "title": "Build \"v2\"" }
            })
        );
    }

    #[test]
    fn previews_first_line_of_long_heredoc_command() {
        let mut p = PartialJsonFields::default();
        assert!(p.feed(r#"{"command":"cat <<'EOF' > big.py\nprint('hello')\n"#));
        assert_eq!(p.value()["command"], "cat <<'EOF' > big.py");
        // Rest of huge command doesn't panic or spam updates.
        assert!(!p.feed(&"x".repeat(2000)));
        assert!(p.feed(r#"","description":"Write python script"}"#));
        assert_eq!(p.value()["description"], "Write python script");
    }

    #[test]
    fn handles_byte_by_byte_streaming() {
        let mut p = PartialJsonFields::default();
        let payload = r#"{"file_path":"C:\\Users\\dane\\foo.rs","description":"Edit file"}"#;
        let mut updates = 0;
        for ch in payload.chars() {
            let mut buf = [0u8; 4];
            if p.feed(ch.encode_utf8(&mut buf)) {
                updates += 1;
            }
        }
        assert_eq!(updates, 2);
        assert_eq!(p.value(), json!({ "file_path": "C:\\Users\\dane\\foo.rs", "description": "Edit file" }));
    }
}
