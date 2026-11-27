//! Remote files over Winfer Connect: browsing, reading, writing, resolving and diffing files on
//! paired machines (`file.call` → `ack`). Both the desktop app and `winferd` share this module,
//! so any paired machine (desktop or server) can answer `file.call` requests.

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::LazyLock;
use std::time::{Duration, Instant, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ENTRIES: usize = 1000;
/// Biggest text file served over Connect (matches `src-tauri/src/files.rs`).
const MAX_TEXT: u64 = 8 * 1024 * 1024;
/// Biggest whole binary file (image, PDF) served over Connect.
const MAX_BYTES: u64 = 32 * 1024 * 1024;
/// Biggest single range chunk (video streaming).
const MAX_RANGE_CHUNK: u64 = 4 * 1024 * 1024;

pub const REMOTE_PREFIX: &str = "remote:";

/// Encodes a file on paired machine `peer` as a viewer document key.
pub fn remote_doc(peer: &str, path: &str) -> String {
    format!("{REMOTE_PREFIX}{peer}:{path}")
}

/// Splits a viewer document key into `(Some(peer), remote_path)` for remote files,
/// or `(None, local_path)` for local files.
pub fn split_doc(doc: &str) -> (Option<&str>, &str) {
    if let Some(rest) = doc.strip_prefix(REMOTE_PREFIX)
        && let Some((peer, path)) = rest.split_once(':')
        && !peer.is_empty()
        && !path.is_empty()
    {
        return (Some(peer), path);
    }
    (None, doc)
}

pub fn is_remote(doc: &str) -> bool {
    split_doc(doc).0.is_some()
}

/// Joins a child entry `name` onto a remote folder path, respecting Windows or POSIX separators.
pub fn join_remote_path(dir: &str, name: &str) -> String {
    let sep = if dir.contains('\\') && !dir.contains('/') { '\\' } else { '/' };
    if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}{sep}{name}")
    }
}

/// Parent directory of a remote path (works on both POSIX and Windows paths regardless of host OS).
pub fn remote_parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    let idx = trimmed.rfind(['/', '\\'])?;
    if idx == 0 {
        return Some(trimmed[..1].to_string());
    }
    // Windows drive root (`C:\foo` → `C:\`).
    if idx == 2 && trimmed.as_bytes()[1] == b':' {
        return Some(trimmed[..3].to_string());
    }
    Some(trimmed[..idx].to_string())
}

/// Resolves a relative link `rel` against a remote document's path, cleaning `.` and `..`.
pub fn resolve_remote_link(doc_path: &str, href_path: &str) -> Option<String> {
    let is_win = doc_path.contains('\\') && !doc_path.contains('/');
    let sep = if is_win { '\\' } else { '/' };
    let is_abs = href_path.starts_with('/')
        || href_path.starts_with('\\')
        || href_path.starts_with("~/")
        || href_path == "~"
        || (href_path.len() >= 3 && href_path.as_bytes()[1] == b':' && matches!(href_path.as_bytes()[2], b'/' | b'\\'));
    let combined = if is_abs {
        href_path.to_string()
    } else {
        let base = remote_parent(doc_path)?;
        join_remote_path(&base, href_path)
    };
    Some(clean_remote_path(&combined, sep))
}

fn clean_remote_path(path: &str, sep: char) -> String {
    let bytes = path.as_bytes();
    let (prefix, rest) = if path.starts_with("~/") {
        ("~/", &path[2..])
    } else if path == "~" {
        return "~".to_string();
    } else if bytes.len() >= 2 && bytes[1] == b':' {
        let end = if bytes.len() >= 3 && matches!(bytes[2], b'/' | b'\\') { 3 } else { 2 };
        (&path[..end], &path[end..])
    } else if path.starts_with('/') || path.starts_with('\\') {
        (&path[..1], &path[1..])
    } else {
        ("", path)
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                if !parts.is_empty() && parts.last() != Some(&"..") {
                    parts.pop();
                } else if prefix.is_empty() {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let sep_str = sep.to_string();
    format!("{prefix}{}", parts.join(&sep_str))
}

/// Summary of a paired machine's workspace (cached from its `panes` mirror for grouping & jumping).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteWorkspaceMeta {
    pub id: String,
    pub name: String,
    pub root_dir: String,
}

type Answer = async_channel::Sender<Result<Value, String>>;

#[derive(Default)]
struct Shared {
    seq: u64,
    /// Outgoing `file.call` requests awaiting an `ack`: rid → (peer, deadline, reply sender).
    waiting: HashMap<String, (String, Instant, Answer)>,
    /// Paired machines' workspaces from their latest `panes` mirror.
    workspaces: HashMap<String, Vec<RemoteWorkspaceMeta>>,
    sweeping: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(Default::default);

/// Stores a paired machine's workspaces from its latest `panes` mirror.
pub fn set_workspaces(peer: &str, workspaces: Vec<RemoteWorkspaceMeta>) {
    SHARED.lock().workspaces.insert(peer.to_string(), workspaces);
}

/// Returns the cached workspaces of a paired machine.
pub fn workspaces_of(peer: &str) -> Vec<RemoteWorkspaceMeta> {
    SHARED.lock().workspaces.get(peer).cloned().unwrap_or_default()
}

/// Calls `op` on `peer`'s filesystem and awaits its answer.
pub async fn call(peer: &str, op: &str, mut args: Value) -> Result<Value, String> {
    let online = crate::connect::get().is_some_and(|c| c.is_up(peer));
    if !online {
        let name = crate::connect::peer_name(peer).unwrap_or_else(|| "That machine".into());
        return Err(format!("{name} is offline."));
    }
    let (tx, rx) = async_channel::bounded(1);
    let rid = {
        let mut s = SHARED.lock();
        s.seq += 1;
        let rid = format!("f{}", s.seq);
        s.waiting.insert(rid.clone(), (peer.to_string(), Instant::now() + TIMEOUT, tx));
        if !std::mem::replace(&mut s.sweeping, true) {
            std::thread::spawn(sweep);
        }
        rid
    };
    if !args.is_object() {
        args = json!({});
    }
    args["t"] = json!("file.call");
    args["op"] = json!(op);
    args["id"] = json!(rid);
    crate::connect::send(peer, args);
    rx.recv().await.map_err(|_| "No answer from remote machine.".to_string())?
}

fn sweep() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let late: Vec<Answer> = {
            let mut s = SHARED.lock();
            let now = Instant::now();
            let ids: Vec<String> = s.waiting.iter().filter(|(_, (_, by, _))| now > *by).map(|(id, _)| id.clone()).collect();
            ids.iter().filter_map(|id| s.waiting.remove(id)).map(|(_, _, tx)| tx).collect()
        };
        for tx in late {
            let _ = tx.try_send(Err("The machine didn't answer (it may need a newer winfer).".into()));
        }
    }
}

/// Intercepts `file.call` (host) and `f*` `ack` (client) messages on the Connect channel.
pub fn intercept(peer: &str, msg: &Value) -> bool {
    match msg["t"].as_str().unwrap_or_default() {
        "file.call" => {
            on_request(peer, msg);
            true
        }
        "ack" => {
            let Some(id) = msg["id"].as_str().filter(|id| id.starts_with('f')) else { return false };
            let Some((_, _, tx)) = SHARED.lock().waiting.remove(id) else { return false };
            let _ = tx.try_send(if msg["ok"] == true {
                Ok(msg["value"].clone())
            } else {
                Err(msg["error"].as_str().unwrap_or("Remote file request failed.").to_string())
            });
            true
        }
        _ => false,
    }
}

/// Fails any in-flight file requests to `peer` when its link drops.
pub fn peer_down(peer: &str) {
    let calls: Vec<Answer> = {
        let mut s = SHARED.lock();
        let ids: Vec<String> = s.waiting.iter().filter(|(_, (p, _, _))| p == peer).map(|(id, _)| id.clone()).collect();
        ids.iter().filter_map(|id| s.waiting.remove(id)).map(|(_, _, tx)| tx).collect()
    };
    let name = crate::connect::peer_name(peer).unwrap_or_else(|| "the machine".into());
    for tx in calls {
        let _ = tx.try_send(Err(format!("Lost the link to {name}.")));
    }
}

// ---- host side ----

fn on_request(peer: &str, msg: &Value) {
    let (peer, msg) = (peer.to_string(), msg.clone());
    std::thread::spawn(move || {
        let answer = match handle(&msg) {
            Ok(value) => json!({ "t": "ack", "id": msg["id"], "ok": true, "value": value }),
            Err(error) => json!({ "t": "ack", "id": msg["id"], "ok": false, "error": error }),
        };
        crate::connect::send(&peer, answer);
    });
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<u64> {
    Some(meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn file_modified(path: &Path) -> Option<u64> {
    modified_ms(&std::fs::metadata(path).ok()?)
}

fn git(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.output().map_err(|e| format!("Couldn't run git: {e}"))
}

/// Executes a single `file.call` operation on the local filesystem.
pub fn handle(msg: &Value) -> Result<Value, String> {
    let op = msg["op"].as_str().unwrap_or_default();
    match op {
        "list" => handle_list(msg["path"].as_str().unwrap_or("~")),
        "stat" => handle_stat(msg["path"].as_str().unwrap_or_default()),
        "read_text" => handle_read_text(msg["path"].as_str().unwrap_or_default()),
        "write_text" => handle_write_text(
            msg["path"].as_str().unwrap_or_default(),
            msg["text"].as_str().unwrap_or_default(),
            msg["expected"].as_u64(),
        ),
        "read_bytes" => handle_read_bytes(
            msg["path"].as_str().unwrap_or_default(),
            msg["root"].as_str(),
            msg["offset"].as_u64(),
            msg["len"].as_u64(),
        ),
        "resolve" => handle_resolve(
            msg["cwd"].as_str().unwrap_or("~"),
            msg["path"].as_str().unwrap_or_default(),
            msg["roots"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()),
        ),
        "git_show" => handle_git_show(msg["path"].as_str().unwrap_or_default()),
        other => Err(format!("Unknown file op: {other}")),
    }
}

fn handle_list(path: &str) -> Result<Value, String> {
    let dir = crate::folders::expand(path);
    let dir = dir.canonicalize().unwrap_or(dir);
    let read = std::fs::read_dir(&dir).map_err(|e| format!("Can't open {}: {e}", dir.display()))?;
    let mut entries: Vec<(bool, bool, String, u64, bool)> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".DS_Store" || name == ".git" {
                return None;
            }
            let p = e.path();
            let meta = std::fs::metadata(&p).ok();
            let is_dir = meta.as_ref().map_or_else(|| e.file_type().is_ok_and(|t| t.is_dir()), |m| m.is_dir());
            let size = if is_dir { 0 } else { meta.as_ref().map_or(0, |m| m.len()) };
            let git = is_dir && p.join(".git").exists();
            Some((is_dir, name.starts_with('.'), name, size, git))
        })
        .collect();
    // Folders first, then files; non-hidden before hidden; alphabetical.
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
    });
    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(json!({
        "path": crate::folders::display(&dir),
        "absPath": dir.to_string_lossy(),
        "parent": dir.parent().map(|p| p.to_string_lossy().into_owned()),
        "sep": std::path::MAIN_SEPARATOR_STR,
        "git": dir.join(".git").exists(),
        "entries": entries.into_iter().map(|(is_dir, _, name, size, git)| {
            if is_dir {
                json!({ "name": name, "dir": true, "git": git })
            } else {
                json!({ "name": name, "dir": false, "size": size })
            }
        }).collect::<Vec<_>>(),
        "truncated": truncated,
    }))
}

fn handle_stat(path: &str) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("path is empty".into());
    }
    let p = crate::folders::expand(path);
    match std::fs::metadata(&p) {
        Ok(meta) => {
            let abs = p.canonicalize().unwrap_or(p);
            Ok(json!({
                "exists": true,
                "isFile": meta.is_file(),
                "isDir": meta.is_dir(),
                "size": meta.len(),
                "modified": modified_ms(&meta),
                "absPath": abs.to_string_lossy(),
            }))
        }
        Err(_) => Ok(json!({
            "exists": false,
            "isFile": false,
            "isDir": false,
            "size": 0,
            "modified": Value::Null,
            "absPath": p.to_string_lossy(),
        })),
    }
}

fn handle_read_text(path: &str) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("path is empty".into());
    }
    let p = crate::folders::expand(path);
    let meta = std::fs::metadata(&p).map_err(|e| format!("can't open {}: {e}", p.display()))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a file", p.display()));
    }
    if meta.len() > MAX_TEXT {
        return Err(format!("{} is too big to show ({} MB)", p.display(), meta.len() / 1024 / 1024));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("can't read {}: {e}", p.display()))?;
    Ok(json!({
        "path": p.to_string_lossy(),
        "text": String::from_utf8_lossy(&bytes),
        "modified": modified_ms(&meta).unwrap_or(0),
    }))
}

fn handle_write_text(path: &str, text: &str, expected: Option<u64>) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("path is empty".into());
    }
    let p = crate::folders::expand(path);
    if !p.is_file() {
        return Err(format!("{} isn't a file", p.display()));
    }
    if let Some(expected) = expected
        && file_modified(&p) != Some(expected)
    {
        return Err("changed".into());
    }
    std::fs::write(&p, text).map_err(|e| format!("can't save {}: {e}", p.display()))?;
    Ok(json!({ "modified": file_modified(&p).unwrap_or(0) }))
}

fn handle_read_bytes(path: &str, root: Option<&str>, offset: Option<u64>, len: Option<u64>) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("path is empty".into());
    }
    let p = crate::folders::expand(path);
    if let Some(root) = root {
        let root_real = crate::folders::expand(root).canonicalize().map_err(|_| "Outside the page's folder".to_string())?;
        let file_real = p.canonicalize().map_err(|_| format!("can't open {}", p.display()))?;
        if !file_real.starts_with(&root_real) {
            return Err("Outside the page's folder".into());
        }
    }
    let meta = std::fs::metadata(&p).map_err(|e| format!("can't open {}: {e}", p.display()))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a file", p.display()));
    }
    let total = meta.len();
    let modified = modified_ms(&meta).unwrap_or(0);
    if let Some(offset) = offset {
        let want = len.unwrap_or(MAX_RANGE_CHUNK).min(MAX_RANGE_CHUNK);
        let mut file = std::fs::File::open(&p).map_err(|e| format!("can't open {}: {e}", p.display()))?;
        let start = offset.min(total);
        file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::with_capacity(want.min(total.saturating_sub(start)) as usize);
        file.take(want).read_to_end(&mut bytes).map_err(|e| format!("can't read {}: {e}", p.display()))?;
        return Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
            "offset": start,
            "size": total,
            "modified": modified,
        }));
    }
    if total > MAX_BYTES {
        return Err(format!("{} is too big ({} MB)", p.display(), total / 1024 / 1024));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("can't read {}: {e}", p.display()))?;
    Ok(json!({
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "size": total,
        "modified": modified,
    }))
}

fn handle_resolve(cwd: &str, path: &str, roots: Option<Vec<String>>) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Ok(json!({ "path": Value::Null }));
    }
    let cwd = crate::folders::expand(cwd).to_string_lossy().into_owned();
    let roots = roots.map(|rs| rs.into_iter().map(|r| crate::folders::expand(&r).to_string_lossy().into_owned()).collect());
    let found = tauri::async_runtime::block_on(winfer_backend::files::files_resolve(cwd, vec![path.to_string()], roots))
        .into_iter()
        .next()
        .flatten();
    Ok(json!({ "path": found }))
}

fn handle_git_show(path: &str) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("Not a file.".into());
    }
    let file: PathBuf = crate::folders::expand(path);
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else {
        return Err("Not a file.".into());
    };
    if !git(dir, &["rev-parse", "--is-inside-work-tree"])?.status.success() {
        return Ok(json!({ "state": "not_git" }));
    }
    let shown = git(dir, &["show", &format!("HEAD:./{name}")])?;
    if shown.status.success() {
        Ok(json!({ "state": "committed", "text": String::from_utf8_lossy(&shown.stdout) }))
    } else {
        Ok(json!({ "state": "new" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_doc_round_trips() {
        let key = remote_doc("peer_ABC-123", "/home/dev/app/README.md");
        assert_eq!(key, "remote:peer_ABC-123:/home/dev/app/README.md");
        assert_eq!(split_doc(&key), (Some("peer_ABC-123"), "/home/dev/app/README.md"));
        assert!(is_remote(&key));

        let win = remote_doc("peer1", r"C:\Users\dev\a.rs");
        assert_eq!(split_doc(&win), (Some("peer1"), r"C:\Users\dev\a.rs"));

        assert_eq!(split_doc("/Users/a/b.rs"), (None, "/Users/a/b.rs"));
        assert_eq!(split_doc(r"C:\a\b.rs"), (None, r"C:\a\b.rs"));
        assert!(!is_remote("/Users/a/b.rs"));
    }

    #[test]
    fn resolves_remote_relative_links() {
        assert_eq!(resolve_remote_link("/home/u/repo/docs/a.md", "b.md").as_deref(), Some("/home/u/repo/docs/b.md"));
        assert_eq!(resolve_remote_link("/home/u/repo/docs/a.md", "../src/main.rs").as_deref(), Some("/home/u/repo/src/main.rs"));
        assert_eq!(resolve_remote_link("/home/u/repo/docs/a.md", "/etc/hosts").as_deref(), Some("/etc/hosts"));
        assert_eq!(resolve_remote_link(r"C:\repo\docs\a.md", r"..\src\lib.rs").as_deref(), Some(r"C:\repo\src\lib.rs"));
    }

    #[test]
    fn handles_list_stat_read_write_and_bytes() {
        let dir = std::env::temp_dir().join(format!("winfer-connect-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, "hello world").unwrap();

        let listed = handle(&json!({ "op": "list", "path": dir.to_string_lossy() })).unwrap();
        let entries = listed["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["name"], "sub");
        assert_eq!(entries[0]["dir"], true);
        assert_eq!(entries[1]["name"], "hello.txt");
        assert_eq!(entries[1]["dir"], false);
        assert_eq!(entries[1]["size"], 11);

        let st = handle(&json!({ "op": "stat", "path": file.to_string_lossy() })).unwrap();
        assert_eq!(st["exists"], true);
        assert_eq!(st["isFile"], true);
        let modified = st["modified"].as_u64().unwrap();

        let read = handle(&json!({ "op": "read_text", "path": file.to_string_lossy() })).unwrap();
        assert_eq!(read["text"], "hello world");

        // Conflict when expected doesn't match.
        let conflict = handle(&json!({ "op": "write_text", "path": file.to_string_lossy(), "text": "nope", "expected": modified + 1 }));
        assert_eq!(conflict.unwrap_err(), "changed");

        // Succeeds when expected matches.
        let wrote = handle(&json!({ "op": "write_text", "path": file.to_string_lossy(), "text": "updated", "expected": modified })).unwrap();
        assert!(wrote["modified"].as_u64().is_some());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "updated");

        // Range read + root confinement check.
        let part = handle(&json!({ "op": "read_bytes", "path": file.to_string_lossy(), "root": dir.to_string_lossy(), "offset": 2, "len": 4 })).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD.decode(part["data"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, b"date");
        assert_eq!(part["size"], 7);

        let outside = handle(&json!({ "op": "read_bytes", "path": file.to_string_lossy(), "root": dir.join("sub").to_string_lossy() }));
        assert!(outside.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
