//! Turns file names in a reply's inline code (`` `src/main.rs:42` ``) into links, so a click opens
//! them in the file viewer. Only names that exist on disk become links.

use std::path::{Path, PathBuf};

/// `name` or `name:line` as an existing file: its absolute path and the line.
pub fn resolve(token: &str, cwd: Option<&str>) -> Option<(PathBuf, Option<usize>)> {
    if token.is_empty() || token.len() > 300 || token.contains(char::is_whitespace) || !(token.contains('.') || token.contains('/')) {
        return None;
    }
    let (name, line) = match token.rsplit_once(':') {
        Some((n, l)) if l.parse::<usize>().is_ok() => (n, l.parse().ok()),
        _ => (token, None),
    };
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
            match resolve(token, cwd) {
                Some((path, line_no)) => {
                    let href = format!("{}{}", path.display(), line_no.map_or(String::new(), |l| format!(":{l}")));
                    out.push_str(&format!("[`{token}`](<{href}>)"));
                }
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
}
