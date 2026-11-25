//! A background task's end note (`src-tauri/src/harness/bg.rs` `task_event`), split for a compact
//! one-line row (port of `src/features/code/bgNote.ts`).

#[derive(Debug, Clone, PartialEq)]
pub struct BgNote {
    pub name: String,
    pub cmd: String,
    /// Exit code; `None` when the user stopped the task.
    pub code: Option<i64>,
    pub secs: Option<u64>,
    /// The output since the agent's last read (empty when there was none).
    pub output: String,
}

/// `bg1 "name" (`cmd`)` or `bg1 (`cmd`)` → (name, cmd, rest after the closing `)`/marker).
fn mention<'a>(s: &'a str, end: &str) -> Option<(String, String, &'a str)> {
    let s = s.trim_start();
    let (_id, rest) = s.split_once(' ')?;
    let (name, rest) = match rest.strip_prefix('"') {
        Some(r) => {
            let (name, r) = r.split_once("\" (`")?;
            (name.to_string(), r)
        }
        None => (String::new(), rest.strip_prefix("(`")?),
    };
    let i = rest.find(end)?;
    Some((name, rest[..i].to_string(), &rest[i + end.len()..]))
}

/// Parses a background task note; `None` for any other notice.
pub fn parse(text: &str) -> Option<BgNote> {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("Background task ") {
        let (name, cmd, rest) = mention(rest, "`) exited with code ")?;
        let (code, rest) = rest.split_once(" after ")?;
        let (secs, out) = rest.split_once("s.")?;
        let out = out.trim();
        let out = out.strip_prefix("Its output since your last read:").unwrap_or(out).trim();
        let out = if out == "No new output." { "" } else { out };
        return Some(BgNote { name, cmd, code: Some(code.trim().parse().ok()?), secs: Some(secs.trim().parse().ok()?), output: out.to_string() });
    }
    if let Some(rest) = t.strip_prefix("The user stopped background task ") {
        let (name, cmd, _) = mention(rest, "`)")?;
        return Some(BgNote { name, cmd, code: None, secs: None, output: String::new() });
    }
    None
}

impl BgNote {
    /// The one-line summary: `Verify Azure SKU · done · 9s`.
    pub fn title(&self) -> String {
        let label = if !self.name.is_empty() { self.name.clone() } else { self.cmd.lines().next().unwrap_or("").to_string() };
        match (self.code, self.secs) {
            (Some(0), Some(s)) => format!("{label} · done · {s}s"),
            (Some(c), Some(s)) => format!("{label} · exit {c} · {s}s"),
            _ => format!("{label} · stopped"),
        }
    }

    pub fn failed(&self) -> bool {
        self.code.is_some_and(|c| c != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exit_notes() {
        let n = parse("Background task bg11 \"Verify SKU\" (`az x`) exited with code 0 after 9s. Its output since your last read:\n{\n  \"a\": 1\n}").unwrap();
        assert_eq!((n.name.as_str(), n.cmd.as_str(), n.code, n.secs), ("Verify SKU", "az x", Some(0), Some(9)));
        assert_eq!(n.output, "{\n  \"a\": 1\n}");
        assert_eq!(n.title(), "Verify SKU · done · 9s");
        let n = parse("Background task bg1 (`npm run dev`) exited with code 1 after 4s. No new output.").unwrap();
        assert_eq!((n.name.as_str(), n.output.as_str(), n.failed()), ("", "", true));
        assert_eq!(n.title(), "npm run dev · exit 1 · 4s");
    }

    #[test]
    fn parses_stops_and_ignores_others() {
        let n = parse("The user stopped background task bg2 \"dev\" (`pnpm dev`).").unwrap();
        assert_eq!((n.code, n.title().as_str()), (None, "dev · stopped"));
        assert!(parse("Resumed session").is_none());
    }
}
