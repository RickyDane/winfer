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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_count_changes() {
        let d = unified("x.txt", "a\nb\nc\n", "a\nB\nc\nd\n");
        assert!(d.contains("-b") && d.contains("+B") && d.contains("+d"));
        assert_eq!(stat(&d), (2, 1));
    }
}
