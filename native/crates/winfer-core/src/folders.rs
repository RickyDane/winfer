//! This machine's folders for other machines (Winfer Connect): listing them for the "New
//! workspace on ‹machine›" dialog, cloning a repository there, and the machine's deploy key.

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
    let out = cmd.arg("--").arg(url.trim()).arg(&dest).env("GIT_TERMINAL_PROMPT", "0").env("GIT_SSH_COMMAND", ssh_command()).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        // Later fetches and pulls there (by agents too) offer the deploy key as the clone did.
        let ssh = url.trim().starts_with("git@") || url.trim().starts_with("ssh://");
        if ssh && key_path().exists() {
            let _ = std::process::Command::new("git").arg("-C").arg(&dest).args(["config", "core.sshCommand", &ssh_command()]).output();
        }
        Ok(dest)
    } else {
        let _ = std::fs::remove_dir_all(&dest);
        Err(String::from_utf8_lossy(&out.stderr).lines().rfind(|l| !l.trim().is_empty()).unwrap_or("git clone failed").to_string())
    }
}

/// This server's deploy key (made on first use), which git over ssh offers besides the user's own.
fn key_path() -> PathBuf {
    crate::backend::data_dir().join("ssh").join("id_ed25519")
}

/// git's ssh: never asks (no one is at this terminal), and offers the deploy key when there is one.
pub fn ssh_command() -> String {
    let key = key_path();
    let identity = if key.exists() { format!(" -i '{}'", key.display()) } else { String::new() };
    format!("ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new{identity}")
}

/// The deploy key's public half, to add to a private repository (read access) so this server
/// can clone it; made on first use.
pub fn deploy_key(name: &str) -> Result<String, String> {
    let key = key_path();
    if !key.exists() {
        let dir = key.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let out = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("winferd@{name}"), "-f"]).arg(&key).output().map_err(|e| format!("ssh-keygen: {e} (is openssh-client installed?)"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    }
    std::fs::read_to_string(key.with_extension("pub")).map(|k| k.trim().to_string()).map_err(|e| e.to_string())
}
