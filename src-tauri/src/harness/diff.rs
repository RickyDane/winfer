//! Unified diffs for file edits (shown in approvals and tool cards).

/// A unified diff of `old` → `new` for `path`, with 3 lines of context.
pub fn unified(path: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// Lines added and removed in a unified diff.
pub fn stat(diff: &str) -> (usize, usize) {
    let mut add = 0;
    let mut del = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            add += 1;
        } else if line.starts_with('-') {
            del += 1;
        }
    }
    (add, del)
}

/// Undoes a unified `diff` on `current` (the file after the diff was applied): each hunk's new
/// side is found in order and replaced by its old side. None when a hunk no longer matches.
pub fn reverse(current: &str, diff: &str) -> Option<String> {
    struct Hunk<'a> {
        start: usize,
        old: Vec<&'a str>,
        new: Vec<&'a str>,
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            // "@@ -a,b +c,d @@": the new side starts at line c (1-based).
            let start = rest.split_whitespace().find_map(|t| t.strip_prefix('+')).and_then(|t| t.split(',').next()?.parse::<usize>().ok()).unwrap_or(1);
            hunks.push(Hunk { start: start.saturating_sub(1), old: Vec::new(), new: Vec::new() });
            continue;
        }
        let Some(h) = hunks.last_mut() else { continue };
        if let Some(l) = line.strip_prefix('+') {
            h.new.push(l);
        } else if let Some(l) = line.strip_prefix('-') {
            h.old.push(l);
        } else if let Some(l) = line.strip_prefix(' ') {
            h.old.push(l);
            h.new.push(l);
        } else if line.is_empty() {
            h.old.push("");
            h.new.push("");
        }
    }
    if hunks.is_empty() {
        return None;
    }
    let lines: Vec<&str> = current.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut pos = 0;
    for h in &hunks {
        let at = if h.new.is_empty() {
            h.start.clamp(pos, lines.len())
        } else {
            let n = h.new.len();
            let fits = |i: usize| i + n <= lines.len() && lines[i..i + n].iter().zip(&h.new).all(|(a, b)| a.trim_end_matches('\r') == b.trim_end_matches('\r'));
            // Prefer the spot the header names, then the first match after the previous hunk.
            if h.start >= pos && fits(h.start) { h.start } else { (pos..lines.len()).find(|&i| fits(i))? }
        };
        out.extend(&lines[pos..at]);
        out.extend(&h.old);
        pos = at + h.new.len();
    }
    out.extend(&lines[pos..]);
    let mut text = out.join("\n");
    if !text.is_empty() && (current.ends_with('\n') || current.is_empty()) {
        text.push('\n');
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_count_changes() {
        let d = unified("x.txt", "a\nb\nc\n", "a\nB\nc\nd\n");
        assert!(d.contains("-b") && d.contains("+B") && d.contains("+d"));
        assert_eq!(stat(&d), (2, 1));
    }

    #[test]
    fn reverses_diffs() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\nj\nk\n";
        assert_eq!(reverse(new, &unified("x", old, new)).as_deref(), Some(old));
        // Diffs without file headers (as Codex sends them) work too.
        assert_eq!(reverse("x\ny\n", "@@ -1,2 +1,2 @@\n x\n-z\n+y\n").as_deref(), Some("x\nz\n"));
        assert_eq!(reverse("other\n", "@@ -1 +1 @@\n-a\n+b\n"), None);
    }
}
