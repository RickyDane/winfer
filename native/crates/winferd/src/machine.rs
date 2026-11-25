//! The server's folders and health, for the desktop's "New workspace on ‹server›" dialog and the
//! server's row: `fs.list`, `git.clone`, and `server.stats` in the mirror.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_ENTRIES: usize = 500;

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// `~`, `~/x` and relative paths (to home) made absolute.
pub fn expand(path: &str) -> PathBuf {
    let path = path.trim();
    if path.is_empty() || path == "~" {
        return home();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest);
    }
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { home().join(p) }
}

/// Shown with `~` for home, as the desktop does.
pub fn display(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The folders in a folder (hidden ones last), and whether each is a git repository.
pub fn list(path: &str) -> Value {
    let dir = expand(path);
    let dir = dir.canonicalize().unwrap_or(dir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return json!({"t": "fs", "path": display(&dir), "error": e.to_string()}),
    };
    let mut dirs: Vec<(bool, String, bool)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) || e.path().is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (name.starts_with('.'), name, e.path().join(".git").exists())
        })
        .collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let truncated = dirs.len() > MAX_ENTRIES;
    dirs.truncate(MAX_ENTRIES);
    json!({
        "t": "fs", "path": display(&dir), "parent": dir.parent().map(display),
        "git": dir.join(".git").exists(),
        "dirs": dirs.into_iter().map(|(_, name, git)| json!({"name": name, "git": git})).collect::<Vec<_>>(),
        "truncated": truncated,
    })
}

/// The folder a clone of `url` gets: its last path segment without `.git`.
pub fn repo_name(url: &str) -> Option<String> {
    let last = url.trim().trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != ".." && !name.contains(['\\', '\0'])).then(|| name.to_string())
}

pub fn looks_like_git_url(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("https://") || t.starts_with("http://") || t.starts_with("ssh://") || t.starts_with("git@") || t.starts_with("git://")) && !t.contains(char::is_whitespace)
}

/// Clones `url` (optionally a `branch`) into `parent`; returns the new folder.
pub fn clone(url: &str, branch: Option<&str>, parent: &str) -> Result<PathBuf, String> {
    if !looks_like_git_url(url) {
        return Err("That isn't a git URL.".into());
    }
    let name = repo_name(url).ok_or("Can't name a folder after that URL.")?;
    let parent = expand(parent);
    std::fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let dest = parent.join(&name);
    if dest.exists() {
        return Err(format!("{} already exists.", display(&dest)));
    }
    let mut cmd = Command::new("git");
    cmd.arg("clone");
    if let Some(b) = branch.filter(|b| !b.trim().is_empty()) {
        cmd.args(["--branch", b.trim()]);
    }
    // Never stop to ask for a password: there's no one at this terminal.
    let out = cmd.arg("--").arg(url.trim()).arg(&dest).env("GIT_TERMINAL_PROMPT", "0").env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new").output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(dest)
    } else {
        let _ = std::fs::remove_dir_all(&dest);
        Err(String::from_utf8_lossy(&out.stderr).lines().rfind(|l| !l.trim().is_empty()).unwrap_or("git clone failed").to_string())
    }
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Load, memory, disk and uptime, where the OS tells (Linux: /proc; elsewhere: what `df` and
/// `sysctl` give).
pub fn stats() -> Value {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let load = read("/proc/loadavg").and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok()).or_else(|| {
        let out = Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().trim_start_matches('{').split_whitespace().next()?.parse().ok()
    });
    let mem = read("/proc/meminfo").map(|t| {
        let field = |name: &str| t.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 1024;
        (field("MemTotal:"), field("MemTotal:").saturating_sub(field("MemAvailable:")))
    });
    let uptime = read("/proc/uptime").and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok()).map(|s| s as u64);
    let disk = Command::new("df").args(["-kP"]).arg(home()).output().ok().and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let cols: Vec<u64> = text.lines().nth(1)?.split_whitespace().skip(1).take(3).filter_map(|c| c.parse().ok()).collect();
        (cols.len() == 3).then(|| (cols[0] * 1024, cols[1] * 1024))
    });
    json!({
        "cpus": cpus, "load": load,
        "memTotal": mem.map(|m| m.0), "memUsed": mem.map(|m| m.1),
        "diskTotal": disk.map(|d| d.0), "diskUsed": disk.map(|d| d.1),
        "uptime": uptime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_clones_after_the_repo() {
        assert_eq!(repo_name("https://github.com/RickyDane/winfer.git").as_deref(), Some("winfer"));
        assert_eq!(repo_name("git@github.com:org/tool.git").as_deref(), Some("tool"));
        assert_eq!(repo_name("https://example.com/x/y/").as_deref(), Some("y"));
        assert_eq!(repo_name("https://example.com/.."), None);
        assert!(looks_like_git_url("git@github.com:org/tool.git"));
        assert!(!looks_like_git_url("~/code"));
    }

    #[test]
    fn lists_folders_with_home_as_tilde() {
        let v = list("~");
        assert_eq!(v["path"], "~");
        assert!(v["dirs"].is_array());
        assert!(stats()["cpus"].as_u64().unwrap() >= 1);
    }
}
