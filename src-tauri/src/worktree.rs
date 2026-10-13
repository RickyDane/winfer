//! One git worktree per agent pane, so parallel agents never edit the same checkout.
//!
//! Worktrees live under the app data dir (`worktrees/<repo>-<hash>/<provider>-<n>`) on branches
//! named `winfer/<provider>-<n>`, created from the repo's current branch.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::env::login_env;
use crate::pty::resolve_cwd;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub repo: String,
    pub path: String,
    pub branch: String,
    pub base: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDiff {
    /// Unified diff of committed + uncommitted changes since the branch point.
    pub diff: String,
    pub untracked: Vec<String>,
    /// Commits on the worktree branch that aren't on the base.
    pub ahead: u32,
    /// Uncommitted changes (tracked or untracked) in the worktree.
    pub dirty: bool,
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(login_env())
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// Creates a worktree for `provider` in the repo containing `dir`. `Ok(None)` if not a git repo.
pub fn create(
    worktrees_dir: &Path,
    dir: &Path,
    provider: &str,
) -> Result<Option<WorktreeInfo>, String> {
    let Some(repo) = repo_root(dir) else {
        return Ok(None);
    };

    let base = match git(&repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(branch) => branch,
        // Detached HEAD: branch from the commit itself.
        Err(_) => git(&repo, &["rev-parse", "HEAD"])?,
    };

    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let mut hasher = DefaultHasher::new();
    repo.hash(&mut hasher);
    let parent = worktrees_dir.join(format!("{repo_name}-{:08x}", hasher.finish() as u32));

    let (branch, path) = (1..)
        .map(|n| {
            (
                format!("winfer/{provider}-{n}"),
                parent.join(format!("{provider}-{n}")),
            )
        })
        .find(|(branch, path)| !branch_exists(&repo, branch) && !path.exists())
        .expect("unbounded range");

    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            &base,
        ],
    )?;

    Ok(Some(WorktreeInfo {
        repo: repo.to_string_lossy().into_owned(),
        path: path.to_string_lossy().into_owned(),
        branch,
        base,
    }))
}

pub fn diff(info: &WorktreeInfo) -> Result<WorktreeDiff, String> {
    let path = Path::new(&info.path);
    let merge_base = git(path, &["merge-base", &info.base, "HEAD"])?;
    // Diffing against the merge base with no second commit compares to the working tree,
    // so committed and uncommitted changes show together.
    let diff = git(path, &["diff", "--no-color", &merge_base])?;
    let untracked = git(path, &["ls-files", "--others", "--exclude-standard"])?
        .lines()
        .map(String::from)
        .collect::<Vec<_>>();
    let ahead = git(
        path,
        &["rev-list", "--count", &format!("{}..HEAD", info.base)],
    )?
    .parse()
    .unwrap_or(0);
    let dirty = !git(path, &["status", "--porcelain"])?.is_empty();
    Ok(WorktreeDiff {
        diff,
        untracked,
        ahead,
        dirty,
    })
}

/// Merges the worktree branch into its base in the main checkout. Refuses instead of
/// guessing when either side has uncommitted changes; aborts cleanly on conflicts.
pub fn merge(info: &WorktreeInfo) -> Result<String, String> {
    let path = Path::new(&info.path);
    let repo = Path::new(&info.repo);

    if !git(path, &["status", "--porcelain"])?.is_empty() {
        return Err(
            "The worktree has uncommitted changes. Ask the agent to commit them first.".into(),
        );
    }
    let current = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default();
    if current != info.base {
        return Err(format!(
            "The main checkout is on “{current}”, not “{}”. Switch it back first.",
            info.base
        ));
    }
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err(
            "The main checkout has uncommitted changes. Commit or stash them first.".into(),
        );
    }

    match git(repo, &["merge", "--no-ff", "--no-edit", &info.branch]) {
        Ok(out) => Ok(out),
        Err(err) => {
            let _ = git(repo, &["merge", "--abort"]);
            Err(format!("Merge failed and was aborted:\n{err}"))
        }
    }
}

pub fn remove(info: &WorktreeInfo, delete_branch: bool) -> Result<(), String> {
    let repo = Path::new(&info.repo);
    git(repo, &["worktree", "remove", "--force", &info.path])?;
    if delete_branch {
        git(repo, &["branch", "-D", &info.branch])?;
    }
    Ok(())
}

fn worktrees_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("worktrees"))
}

#[tauri::command]
pub async fn worktree_create(
    app: AppHandle,
    dir: String,
    provider: String,
) -> Result<Option<WorktreeInfo>, String> {
    create(&worktrees_dir(&app)?, &resolve_cwd(Some(&dir)), &provider)
}

#[tauri::command]
pub async fn worktree_diff(info: WorktreeInfo) -> Result<WorktreeDiff, String> {
    diff(&info)
}

#[tauri::command]
pub async fn worktree_merge(info: WorktreeInfo) -> Result<String, String> {
    merge(&info)
}

#[tauri::command]
pub async fn worktree_remove(info: WorktreeInfo, delete_branch: bool) -> Result<(), String> {
    remove(&info, delete_branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, script: &str) {
        let ok = Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "failed: {script}");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("winfer-test-{name}-{:x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn repo() -> PathBuf {
        let dir = temp_dir("repo");
        sh(
            &dir,
            "git init -q -b main && git config user.email t@t && git config user.name t \
             && echo one > a.txt && git add . && git commit -qm init",
        );
        dir
    }

    #[test]
    fn non_repo_returns_none() {
        assert_eq!(
            create(&temp_dir("wt"), &temp_dir("plain"), "claude").unwrap(),
            None
        );
    }

    #[test]
    fn create_diff_merge_remove() {
        let repo = repo();
        let wts = temp_dir("wt");

        let a = create(&wts, &repo, "claude").unwrap().unwrap();
        let b = create(&wts, &repo, "claude").unwrap().unwrap();
        assert_eq!(
            (a.branch.as_str(), b.branch.as_str()),
            ("winfer/claude-1", "winfer/claude-2")
        );
        assert_eq!(a.base, "main");

        // Uncommitted + untracked changes show up and block merging.
        let path = PathBuf::from(&a.path);
        sh(&path, "echo two >> a.txt && echo new > b.txt");
        let d = diff(&a).unwrap();
        assert!(d.diff.contains("+two"), "{}", d.diff);
        assert_eq!(d.untracked, vec!["b.txt"]);
        assert!(d.dirty);
        assert!(merge(&a).unwrap_err().contains("uncommitted"));

        // Once committed, it merges into main.
        sh(
            &path,
            "git add . && git -c user.email=t@t -c user.name=t commit -qm work",
        );
        assert_eq!(diff(&a).unwrap().ahead, 1);
        merge(&a).unwrap();
        sh(&repo, "grep -q two a.txt && test -f b.txt");

        remove(&a, true).unwrap();
        assert!(!path.exists());
        assert!(!branch_exists(&repo, "winfer/claude-1"));
        remove(&b, true).unwrap();
    }

    #[test]
    fn merge_conflict_is_aborted() {
        let repo = repo();
        let a = create(&temp_dir("wt"), &repo, "codex").unwrap().unwrap();
        sh(
            Path::new(&a.path),
            "echo theirs > a.txt && git -c user.email=t@t -c user.name=t commit -qam theirs",
        );
        sh(&repo, "echo ours > a.txt && git commit -qam ours");

        assert!(merge(&a).unwrap_err().contains("aborted"));
        // Main checkout is left clean, not mid-merge.
        sh(
            &repo,
            "test -z \"$(git status --porcelain)\" && grep -q ours a.txt",
        );
    }
}
