//! Local files winfer shows itself: the markdown viewer, and file paths made clickable in terminals.
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Biggest document the viewer opens.
const MAX_TEXT: u64 = 8 * 1024 * 1024;
/// Biggest image a document may embed.
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// `path` (absolute, `~/…`, or relative to `cwd`) as a clean absolute path, without touching the disk.
fn resolve(cwd: &str, path: &str) -> PathBuf {
    let path = crate::editors::expand_home(path);
    let full = if path.is_absolute() { path } else { crate::editors::expand_home(cwd).join(path) };
    let mut out = PathBuf::new();
    for part in full.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn modified(path: &Path) -> Option<u64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Folders never worth searching for a file an agent mentioned.
const SKIP_DIRS: &[&str] = &["node_modules", "target", ".git", ".venv", "venv", "__pycache__", ".next", ".turbo", ".cache", "Pods"];
/// How many files a folder search looks at, and how long its listing is reused.
const INDEX_LIMIT: usize = 50_000;
const INDEX_TTL: Duration = Duration::from_secs(15);

type Index = (Instant, Arc<Vec<PathBuf>>);
static INDEXES: OnceLock<Mutex<HashMap<PathBuf, Index>>> = OnceLock::new();

/// Files under `root` (relative to it), skipping dependency and build caches. Cached briefly.
fn index(root: &Path) -> Arc<Vec<PathBuf>> {
    let cache = INDEXES.get_or_init(Default::default);
    if let Some((at, files)) = cache.lock().unwrap().get(root)
        && at.elapsed() < INDEX_TTL
    {
        return files.clone();
    }
    let mut files = Vec::new();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .max_depth(Some(12))
        .filter_entry(|e| !(e.file_type().is_some_and(|t| t.is_dir()) && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())))
        .build();
    for entry in walk.flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            files.push(entry.path().strip_prefix(root).unwrap_or(entry.path()).to_path_buf());
            if files.len() >= INDEX_LIMIT {
                break;
            }
        }
    }
    let files = Arc::new(files);
    let mut map = cache.lock().unwrap();
    map.retain(|_, (at, _)| at.elapsed() < INDEX_TTL);
    map.insert(root.to_path_buf(), (Instant::now(), files.clone()));
    files
}

/// The folder whose files a relative path is searched among: the git checkout `cwd` is in, else `cwd`.
fn search_root(cwd: &Path) -> PathBuf {
    let home = dirs::home_dir();
    for dir in cwd.ancestors() {
        if Some(dir) == home.as_deref() || dir.parent().is_none() {
            break;
        }
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
    }
    cwd.to_path_buf()
}

/// Where a relative `path` most likely is: from `cwd`, from a folder above it up to the git
/// checkout (agents print repo-relative paths from subfolders), from one of `roots`, and lastly
/// any file under the checkout whose path ends with it (agents often print just `index.html`).
fn find(cwd: &str, path: &str, roots: &[String]) -> Option<PathBuf> {
    let direct = resolve(cwd, path);
    if direct.is_file() {
        return Some(direct);
    }
    let rel = Path::new(path);
    if rel.is_absolute() || path.starts_with('~') || rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let cwd = resolve("", cwd);
    if !cwd.is_absolute() {
        return None;
    }
    let top = search_root(&cwd);
    for dir in cwd.ancestors().skip(1) {
        if !dir.starts_with(&top) {
            break;
        }
        let full = resolve(&dir.to_string_lossy(), path);
        if full.is_file() {
            return Some(full);
        }
    }
    for root in roots {
        let full = resolve(root, path);
        if full.is_file() {
            return Some(full);
        }
    }
    let wanted: Vec<_> = rel.components().filter(|c| !matches!(c, Component::CurDir)).collect();
    if wanted.is_empty() {
        return None;
    }
    let files = index(&top);
    let found = files
        .iter()
        .filter(|f| {
            let parts: Vec<_> = f.components().collect();
            parts.len() >= wanted.len() && parts[parts.len() - wanted.len()..] == wanted[..]
        })
        // Closest to the checkout's top wins: `index.html` over `vendor/x/index.html`.
        .min_by_key(|f| f.components().count())?;
    let full = top.join(found);
    full.is_file().then_some(full)
}

/// For each of `paths`, the absolute path of the file it names, or null where there is no such file.
/// Relative paths are looked up from `cwd`, then its parent folders and `roots` (see [`find`]).
#[tauri::command]
pub async fn files_resolve(cwd: String, paths: Vec<String>, roots: Option<Vec<String>>) -> Vec<Option<String>> {
    let roots = roots.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| find(&cwd, p, &roots).map(|full| full.to_string_lossy().into_owned()))
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[derive(Serialize)]
pub struct TextFile {
    path: String,
    text: String,
    /// Last change, in ms since the epoch.
    modified: u64,
}

/// A text file (a markdown document) and when it last changed.
#[tauri::command]
pub async fn files_read_text(path: String) -> Result<TextFile, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = resolve("", &path);
        if !path.is_absolute() {
            return Err(format!("{} isn't an absolute path", path.display()));
        }
        let meta = std::fs::metadata(&path).map_err(|e| format!("can't open {}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} isn't a file", path.display()));
        }
        if meta.len() > MAX_TEXT {
            return Err(format!("{} is too big to show ({} MB)", path.display(), meta.len() / 1024 / 1024));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
        Ok(TextFile {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            modified: modified(&path).unwrap_or(0),
            path: path.to_string_lossy().into_owned(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// When a file last changed (ms since the epoch), or null if it's gone: the viewer polls this to reload.
#[tauri::command]
pub fn files_modified(path: String) -> Option<u64> {
    modified(Path::new(&path))
}

/// Saves `text` to a file the viewer shows (the user's edits and comments) and returns when it
/// changed. With `expected` set, refuses when the file changed since then (an agent edited it).
#[tauri::command]
pub async fn files_write_text(path: String, text: String, expected: Option<u64>) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = resolve("", &path);
        if !path.is_absolute() {
            return Err(format!("{} isn't an absolute path", path.display()));
        }
        if !path.is_file() {
            return Err(format!("{} isn't a file", path.display()));
        }
        if let Some(expected) = expected {
            if modified(&path) != Some(expected) {
                return Err("changed".into());
            }
        }
        std::fs::write(&path, text).map_err(|e| format!("can't save {}: {e}", path.display()))?;
        Ok(modified(&path).unwrap_or(0))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A file's bytes, for images a document embeds.
#[tauri::command]
pub async fn files_read_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = resolve("", &path);
        let len = std::fs::metadata(&path).map_err(|e| format!("can't open {}: {e}", path.display()))?.len();
        if len > MAX_BYTES {
            return Err(format!("{} is too big", path.display()));
        }
        std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("can't read {}: {e}", path.display()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_paths_lexically() {
        let root = if cfg!(windows) { "C:\\work\\repo" } else { "/work/repo" };
        let expect = |p: &str| PathBuf::from(root).join(p);
        assert_eq!(resolve(root, "docs/README.md"), expect("docs/README.md"));
        assert_eq!(resolve(root, "./docs/../PLAN.md"), expect("PLAN.md"));
        let abs = if cfg!(windows) { "D:\\notes\\a.md" } else { "/notes/a.md" };
        assert_eq!(resolve(root, abs), PathBuf::from(abs));
    }

    #[test]
    fn finds_existing_files_only() {
        let dir = std::env::temp_dir().join(format!("winfer-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/a.md"), "# A").unwrap();
        let cwd = dir.to_string_lossy().into_owned();
        let found = tauri::async_runtime::block_on(files_resolve(cwd.clone(), vec!["docs/a.md".into(), "docs/b.md".into(), "docs".into()], None));
        assert_eq!(found[0].as_deref(), Some(dir.join("docs/a.md").to_string_lossy().as_ref()));
        assert_eq!(found[1], None);
        assert_eq!(found[2], None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn finds_files_from_parent_folders_and_by_name() {
        let dir = std::env::temp_dir().join(format!("winfer-find-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("web/out")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
        std::fs::write(dir.join("web/out/index.html"), "<p>").unwrap();
        std::fs::write(dir.join("node_modules/x/page.html"), "<p>").unwrap();
        std::fs::write(dir.join("PLAN.md"), "# P").unwrap();
        let sub = dir.join("web").to_string_lossy().into_owned();
        let found = |p: &str| find(&sub, p, &[]);
        assert_eq!(found("PLAN.md"), Some(dir.join("PLAN.md")));
        assert_eq!(found("index.html"), Some(dir.join("web/out/index.html")));
        assert_eq!(found("out/index.html"), Some(dir.join("web/out/index.html")));
        assert_eq!(found("page.html"), None);
        assert_eq!(found("../nope.md"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
