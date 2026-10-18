//! Local files winfer shows itself: the markdown viewer, and file paths made clickable in terminals.
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

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

/// For each of `paths`, the absolute path of the file it names, or null where there is no such file.
#[tauri::command]
pub async fn files_resolve(cwd: String, paths: Vec<String>) -> Vec<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| {
                let full = resolve(&cwd, p);
                full.is_file().then(|| full.to_string_lossy().into_owned())
            })
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
        let found = tauri::async_runtime::block_on(files_resolve(cwd, vec!["docs/a.md".into(), "docs/b.md".into(), "docs".into()]));
        assert_eq!(found[0].as_deref(), Some(dir.join("docs/a.md").to_string_lossy().as_ref()));
        assert_eq!(found[1], None);
        assert_eq!(found[2], None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
