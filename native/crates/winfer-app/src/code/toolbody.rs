//! What a tool row shows when opened: the command, the file written, the edit as a diff, the
//! arguments and the output, built from the tool call's input (port of the web transcript's body,
//! extended so Write, Edit and plugin tools have content too).

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Section {
    /// A shell command, shown as `$ command`.
    Command(String),
    /// A unified diff.
    Diff(String),
    /// File contents or other text with line numbers.
    Code(String),
    /// `name  value` rows.
    Fields(Vec<(String, String)>),
    /// Plain output.
    Output(String),
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// `old` → `new` as a diff the transcript's diff block draws.
pub fn edit_diff(old: &str, new: &str) -> String {
    let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let mut out = format!("@@ -1,{} +1,{} @@", o.len(), n.len());
    for l in o {
        out.push_str(&format!("\n-{l}"));
    }
    for l in n {
        out.push_str(&format!("\n+{l}"));
    }
    out
}

fn show(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 600 { format!("{}…", s.chars().take(600).collect::<String>()) } else { s }
}

/// Input fields as rows, the long text ones aside (`skip`).
fn fields(input: &Value, skip: &[&str]) -> Vec<(String, String)> {
    let Some(map) = input.as_object() else { return Vec::new() };
    map.iter()
        .filter(|(k, v)| !skip.contains(&k.as_str()) && !v.is_null() && !matches!(v, Value::String(s) if s.is_empty()))
        .map(|(k, v)| (k.replace('_', " "), show(v)))
        .collect()
}

/// One line of tool output that only confirms what the row already says.
fn trivial(output: &str) -> bool {
    let o = output.trim();
    o.is_empty() || (o.lines().count() == 1 && o.len() < 160 && (o.starts_with("File ") || o.contains("successfully") || o.starts_with("The file ")))
}

pub fn sections(name: &str, input: &Value, output: &str, diff: Option<&str>) -> Vec<Section> {
    let mut out = Vec::new();
    let output = output.trim_end();
    if let Some(d) = diff.filter(|d| !d.trim().is_empty()) {
        out.push(Section::Diff(d.to_string()));
    } else {
        match name {
            "Bash" => {
                if let Some(c) = text(input, "command") {
                    out.push(Section::Command(c));
                }
                let f = fields(input, &["command", "timeout", "run_in_background"]);
                if !f.is_empty() {
                    out.push(Section::Fields(f));
                }
            }
            "Write" => {
                if let Some(c) = text(input, "content") {
                    out.push(Section::Code(c));
                }
            }
            "Edit" => {
                if let (Some(o), Some(n)) = (text(input, "old_string"), input.get("new_string").and_then(Value::as_str)) {
                    out.push(Section::Diff(edit_diff(&o, n)));
                }
            }
            "MultiEdit" => {
                let edits: Vec<String> = input.get("edits").and_then(Value::as_array).map(|a| a.iter().filter_map(|e| Some(edit_diff(&text(e, "old_string")?, e.get("new_string")?.as_str()?))).collect()).unwrap_or_default();
                if !edits.is_empty() {
                    out.push(Section::Diff(edits.join("\n")));
                }
            }
            _ => {
                let f = fields(input, &[]);
                if !f.is_empty() {
                    out.push(Section::Fields(f));
                }
            }
        }
    }
    if !output.is_empty() && !(matches!(name, "Write" | "Edit" | "MultiEdit") && trivial(output)) {
        out.push(Section::Output(output.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn write_shows_its_content() {
        let s = sections("Write", &json!({ "file_path": "a.txt", "content": "one\ntwo" }), "File created successfully at: a.txt", None);
        assert_eq!(s, vec![Section::Code("one\ntwo".into())]);
    }

    #[test]
    fn edit_becomes_a_diff() {
        let s = sections("Edit", &json!({ "file_path": "a", "old_string": "x\ny", "new_string": "z" }), "", None);
        assert_eq!(s, vec![Section::Diff("@@ -1,2 +1,1 @@\n-x\n-y\n+z".into())]);
    }

    #[test]
    fn bash_shows_command_then_output() {
        let s = sections("Bash", &json!({ "command": "ls", "description": "List files" }), "a\nb", None);
        assert_eq!(s, vec![Section::Command("ls".into()), Section::Fields(vec![("description".into(), "List files".into())]), Section::Output("a\nb".into())]);
    }

    #[test]
    fn plugin_tools_list_their_arguments() {
        let s = sections("winfer · bg", &json!({ "action": "start", "cmd": "node s.js", "wait": null }), "id 1", None);
        assert_eq!(s, vec![Section::Fields(vec![("action".into(), "start".into()), ("cmd".into(), "node s.js".into())]), Section::Output("id 1".into())]);
    }

    #[test]
    fn a_diff_wins_over_the_input() {
        let s = sections("Write", &json!({ "content": "x" }), "", Some("@@ -1 +1 @@\n-a\n+b"));
        assert_eq!(s.len(), 1);
        assert!(matches!(s[0], Section::Diff(_)));
    }
}
