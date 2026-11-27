//! Turns file names in a reply's inline code (`` `src/main.rs:42` ``) into links, so a click opens
//! them in the file viewer. Only names that exist on disk become links.

use std::path::{Path, PathBuf};

/// Splits a trailing line reference off a file name: `:12`, `:12:5`, `:12-30`, `#L12`, `#L12-L30`
/// and `#L12C3` all give line 12.
pub fn split_line(token: &str) -> (&str, Option<usize>) {
    if let Some((name, frag)) = token.rsplit_once("#L") {
        let digits: String = frag.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(line) = digits.parse() {
            return (name, Some(line));
        }
    }
    let mut name = token;
    let mut line = None;
    // Peel `:N`, `:N-M` and `:N:M` suffixes; the first number is the line.
    while let Some((head, tail)) = name.rsplit_once(':') {
        let first = tail.split('-').next().unwrap_or("");
        if first.is_empty() || !tail.chars().all(|c| c.is_ascii_digit() || c == '-') || !first.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        line = first.parse().ok();
        name = head;
    }
    (name, line)
}

/// `name` or `name:line` as an existing file: its absolute path and the line.
pub fn resolve(token: &str, cwd: Option<&str>) -> Option<(PathBuf, Option<usize>)> {
    if token.is_empty() || token.len() > 300 || token.contains(char::is_whitespace) || !(token.contains('.') || token.contains('/')) {
        return None;
    }
    let (name, line) = split_line(token);
    if name.contains("://") || name.contains(['*', '<', '>', '|', '?']) {
        return None;
    }
    let path = if let Some(rest) = name.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        PathBuf::from(cwd?).join(name.trim_start_matches("./"))
    };
    path.is_file().then_some((path, line))
}

/// The markdown with each existing file in inline code made a link.
pub fn linkify(text: &str, cwd: Option<&str>) -> String {
    linkify_with(text, |token| {
        let (path, line_no) = resolve(token, cwd)?;
        Some(format!("{}{}", path.display(), line_no.map_or(String::new(), |l| format!(":{l}"))))
    })
}

/// Whether an inline code token looks like a file path on a remote machine (where we can't stat
/// synchronously on the local disk; clicking it resolves it over Winfer Connect).
fn looks_like_remote_file(token: &str) -> bool {
    if token.is_empty()
        || token.len() > 300
        || token.contains(char::is_whitespace)
        || token.starts_with('-')
        || token.contains("://")
        || token.contains(['*', '<', '>', '|', '?', '(', ')', '[', ']', '{', '}', '=', ';', ',', '"', '\''])
    {
        return false;
    }
    let (name, _) = split_line(token);
    if name.is_empty() || name.ends_with('/') || name.ends_with('\\') {
        return false;
    }
    let has_sep = name.contains('/') || name.contains('\\');
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let has_ext = base
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=8).contains(&ext.len()) && ext.as_bytes()[0].is_ascii_alphabetic() && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    has_sep || has_ext
}

/// Like [`linkify`], for remote chat panes where files live on another machine.
pub fn linkify_remote(text: &str) -> String {
    linkify_with(text, |token| looks_like_remote_file(token).then(|| token.to_string()))
}

fn linkify_with(text: &str, resolve_href: impl Fn(&str) -> Option<String>) -> String {
    if !text.contains('`') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut fenced = false;
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced || !line.contains('`') {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find('`') {
            let after = &rest[open + 1..];
            // Double backticks and unclosed spans stay as they are.
            let Some(close) = after.find('`').filter(|c| *c > 0 && !after.starts_with('`')) else {
                break;
            };
            let token = &after[..close];
            out.push_str(&rest[..open]);
            match resolve_href(token) {
                Some(href) => out.push_str(&format!("[`{token}`](<{href}>)")),
                None => {
                    out.push('`');
                    out.push_str(token);
                    out.push('`');
                }
            }
            rest = &after[close + 1..];
        }
        out.push_str(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_existing_files_only() {
        let dir = std::env::temp_dir().join(format!("winfer-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        let cwd = dir.to_str().unwrap();
        let out = linkify("Edit `src/main.rs:12`, not `gone.rs` or `cargo build` in:\n```\n`src/main.rs`\n```\nand `src/main.rs`.", Some(cwd));
        let abs = dir.join("src/main.rs");
        assert!(out.contains(&format!("[`src/main.rs:12`](<{}:12>)", abs.display())));
        assert!(out.contains("`gone.rs`") && !out.contains("[`gone.rs`]"));
        assert!(out.contains("`cargo build`") && !out.contains("[`cargo build`]"));
        assert!(out.contains("```\n`src/main.rs`\n```"));
        assert!(out.ends_with(&format!("and [`src/main.rs`](<{}>).", abs.display())));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn splits_line_references() {
        assert_eq!(split_line("a/b.rs"), ("a/b.rs", None));
        assert_eq!(split_line("a/b.rs:12"), ("a/b.rs", Some(12)));
        assert_eq!(split_line("a/b.rs:12:5"), ("a/b.rs", Some(12)));
        assert_eq!(split_line("a/b.rs:12-30"), ("a/b.rs", Some(12)));
        assert_eq!(split_line("a/b.rs#L12"), ("a/b.rs", Some(12)));
        assert_eq!(split_line("a/b.rs#L12-L30"), ("a/b.rs", Some(12)));
        assert_eq!(split_line("a/b.rs#L12C3"), ("a/b.rs", Some(12)));
        assert_eq!(split_line("C:/x/b.rs:7"), ("C:/x/b.rs", Some(7)));
        assert_eq!(split_line("a/b.rs:x"), ("a/b.rs:x", None));
    }

    #[test]
    fn remote_linkify_matches_file_paths_conservatively() {
        let out = linkify_remote("See `src/main.rs:42`, `README.md`, `foo()`, `v1.2`, `--flag`, and `/etc/hosts`.");
        assert!(out.contains("[`src/main.rs:42`](<src/main.rs:42>)"));
        assert!(out.contains("[`README.md`](<README.md>)"));
        assert!(out.contains("[`/etc/hosts`](</etc/hosts>)"));
        assert!(out.contains("`foo()`") && !out.contains("[`foo()`]"));
        assert!(out.contains("`v1.2`") && !out.contains("[`v1.2`]"));
        assert!(out.contains("`--flag`") && !out.contains("[`--flag`]"));
    }
}
