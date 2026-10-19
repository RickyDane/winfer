//! One git worktree per agent pane, so parallel agents never edit the same checkout.
//!
//! Worktrees live under the app data dir (`worktrees/<repo>-<hash>/<provider>-<n>`) on branches
//! named `winfer/<provider>-<n>`, created from the repo's current branch.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

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
    /// Lines in the untracked text files (new files the diff doesn't show).
    pub untracked_lines: u64,
    /// Commits on the worktree branch that aren't on the base.
    pub ahead: u32,
    /// Uncommitted changes (tracked or untracked) in the worktree.
    pub dirty: bool,
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::env::no_window(&mut Command::new("git"))
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

/// Where winfer keeps a repo's worktrees: `<app data>/worktrees/<repo>-<hash>`.
fn worktrees_parent(worktrees_dir: &Path, repo: &Path) -> PathBuf {
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let mut hasher = DefaultHasher::new();
    repo.hash(&mut hasher);
    worktrees_dir.join(format!("{repo_name}-{:08x}", hasher.finish() as u32))
}

/// The repo's main checkout (first in `git worktree list`), even from inside a linked worktree.
fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let top = repo_root(dir)?;
    git(&top, &["worktree", "list", "--porcelain"])
        .ok()
        .and_then(|p| p.lines().next().and_then(|l| l.strip_prefix("worktree ")).map(PathBuf::from))
        .or(Some(top))
}

/// The branch checked out in `repo`'s main checkout (or its commit when detached).
fn current_branch(repo: &Path) -> Result<String, String> {
    git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).or_else(|_| git(repo, &["rev-parse", "HEAD"]))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    /// e.g. "feature/login" or, for remote ones, "origin/feature/login".
    pub name: String,
    pub remote: bool,
    /// Last commit, ms since the epoch.
    pub last_commit_at: Option<u64>,
    /// Folder where it's checked out (main checkout or a worktree), if anywhere.
    pub checked_out_at: Option<String>,
}

/// Local branches (newest first), then remote ones without a local branch of the same name.
pub fn branches(dir: &Path) -> Result<Vec<Branch>, String> {
    let repo = main_checkout(dir).ok_or("not a git repository")?;
    let refs = git(
        &repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%09%(committerdate:unix)%09%(worktreepath)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in refs.lines() {
        let mut parts = line.split('\t');
        let (Some(refname), Some(date), worktree) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let last_commit_at = date.trim().parse::<u64>().ok().map(|s| s * 1000);
        let checked_out_at = worktree.filter(|w| !w.is_empty()).map(String::from);
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            local.push(Branch { name: name.into(), remote: false, last_commit_at, checked_out_at });
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if !name.ends_with("/HEAD") {
                remote.push(Branch { name: name.into(), remote: true, last_commit_at, checked_out_at: None });
            }
        }
    }
    // A remote branch that already has a local twin is reached through the local one.
    let locals: std::collections::HashSet<String> = local.iter().map(|b| b.name.clone()).collect();
    remote.retain(|b| !b.name.split_once('/').is_some_and(|(_, short)| locals.contains(short)));
    local.extend(remote);
    Ok(local)
}

/// A worktree for an existing branch, or — with `new_branch` — for a new branch started from it.
/// Remote branches ("origin/x") get a local tracking branch "x".
pub fn create_from_branch(worktrees_dir: &Path, dir: &Path, branch: &str, new_branch: Option<&str>) -> Result<WorktreeInfo, String> {
    let repo = main_checkout(dir).ok_or("not a git repository")?;
    let base = current_branch(&repo)?;
    let is_remote = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{branch}")]).is_ok()
        && !branch_exists(&repo, branch);
    let new_branch = new_branch.map(str::trim).filter(|b| !b.is_empty());
    if let Some(name) = new_branch {
        git(&repo, &["check-ref-format", "--branch", name]).map_err(|_| format!("“{name}” isn't a valid branch name"))?;
        if branch_exists(&repo, name) {
            return Err(format!("A branch named “{name}” already exists. Pick it from the list instead."));
        }
    }
    let local = match (new_branch, is_remote) {
        (Some(name), _) => name.to_string(),
        (None, true) => branch.split_once('/').map_or(branch, |(_, short)| short).to_string(),
        (None, false) => branch.to_string(),
    };

    let parent = worktrees_parent(worktrees_dir, &repo);
    let leaf = local.replace(['/', '\\', ' '], "-");
    let path = (1..)
        .map(|n| if n == 1 { parent.join(&leaf) } else { parent.join(format!("{leaf}-{n}")) })
        .find(|p| !p.exists())
        .expect("unbounded range");
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy().into_owned();
    match (new_branch, is_remote) {
        (Some(name), _) => git(&repo, &["worktree", "add", "-b", name, &path_str, branch]),
        (None, true) => git(&repo, &["worktree", "add", "--track", "-b", &local, &path_str, branch]),
        (None, false) => git(&repo, &["worktree", "add", &path_str, branch]),
    }
    .map_err(|e| {
        if e.contains("already checked out") || e.contains("is already used by worktree") {
            format!("“{branch}” is already checked out in another folder. Open that worktree from the sidebar instead.")
        } else {
            e
        }
    })?;
    seed_codegraph(&repo, &path);
    Ok(WorktreeInfo {
        repo: repo.to_string_lossy().into_owned(),
        path: path_str,
        branch: local,
        base,
    })
}

/// Files of a codegraph index (`.codegraph/`, gitignored); its daemon's socket and pid stay behind.
const CODEGRAPH_FILES: [&str; 4] = ["codegraph.db", "codegraph.db-wal", "codegraph.db-shm", ".gitignore"];

/// Gives a new worktree a copy of the main checkout's codegraph index, so agents in it get
/// codegraph too (see `mcp::codegraph_server`): `.codegraph/` is gitignored, so `git worktree
/// add` leaves it out. codegraph re-syncs the copy against the worktree's files when its server
/// starts. On APFS (and reflink filesystems) the copy is a clone, instant at any size.
fn seed_codegraph(repo: &Path, worktree: &Path) {
    let (from, to) = (repo.join(".codegraph"), worktree.join(".codegraph"));
    if !from.join("codegraph.db").is_file() || to.exists() {
        return;
    }
    let copied = std::fs::create_dir_all(&to).is_ok()
        && CODEGRAPH_FILES.iter().all(|f| !from.join(f).is_file() || std::fs::copy(from.join(f), to.join(f)).is_ok());
    // A partial index is worse than none: codegraph would trust it.
    if !copied {
        let _ = std::fs::remove_dir_all(&to);
    }
}

/// Updates remote branches (`git fetch --all --prune`).
pub fn fetch(dir: &Path) -> Result<(), String> {
    let repo = main_checkout(dir).ok_or("not a git repository")?;
    git(&repo, &["fetch", "--all", "--prune", "--quiet"]).map(|_| ())
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

    let parent = worktrees_parent(worktrees_dir, &repo);
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;

    // Race and A/B panes start at the same moment, often with the same provider. Picking a
    // free name and claiming it must not interleave, or both pick `-1` and one fails with
    // "already exists". The lock covers this process; the retry covers anything else
    // (another winfer, a manual `git worktree add`) taking the name in between.
    static CREATE: Mutex<()> = Mutex::new(());
    let _guard = CREATE.lock().unwrap_or_else(|e| e.into_inner());

    // Bounded so an unexpected "already exists" can't loop forever.
    for n in 1..1000 {
        let branch = format!("winfer/{provider}-{n}");
        let path = parent.join(format!("{provider}-{n}"));
        if branch_exists(&repo, &branch) || path.exists() {
            continue;
        }
        match git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &path.to_string_lossy(),
                &base,
            ],
        ) {
            Ok(_) => {
                seed_codegraph(&repo, &path);
                return Ok(Some(WorktreeInfo {
                    repo: repo.to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                    branch,
                    base,
                }))
            }
            Err(e) if e.contains("already exists") => continue,
            Err(e) => return Err(e),
        }
    }
    Err(format!("couldn't find a free worktree name for {provider}"))
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
    let untracked_lines = untracked.iter().map(|f| text_lines(&path.join(f))).sum();
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
        untracked_lines,
        ahead,
        dirty,
    })
}

/// Lines in a text file; 0 for binaries, unreadable and very large files.
fn text_lines(file: &Path) -> u64 {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    if std::fs::metadata(file).map_or(true, |m| !m.is_file() || m.len() > MAX_BYTES) {
        return 0;
    }
    match std::fs::read(file) {
        Ok(bytes) if !bytes.contains(&0) => {
            let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
            newlines + u64::from(bytes.last().is_some_and(|&b| b != b'\n'))
        }
        _ => 0,
    }
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

/// Files a worktree changed since it branched off, committed or not (untracked included).
fn changed_files(info: &WorktreeInfo) -> Result<Vec<String>, String> {
    let path = Path::new(&info.path);
    let merge_base = git(path, &["merge-base", &info.base, "HEAD"])?;
    let mut files: Vec<String> = git(path, &["diff", "--name-only", "--no-renames", &merge_base])?
        .lines()
        .chain(git(path, &["ls-files", "--others", "--exclude-standard"])?.lines())
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Files that conflict when `theirs` is merged into `ours` (both commits or branches), without
/// touching any checkout. Empty when it merges cleanly.
fn merge_conflicts(repo: &Path, ours: &str, theirs: &str) -> Result<Vec<String>, String> {
    let out = crate::env::no_window(&mut Command::new("git"))
        .arg("-C")
        .arg(repo)
        .args(["merge-tree", "--write-tree", "--name-only", "--no-messages", ours, theirs])
        .envs(login_env())
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(Vec::new()),
        // Exit 1: conflicts. The tree id comes first, then the conflicted files.
        Some(1) => {
            let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().skip(1).filter(|l| !l.is_empty()).map(String::from).collect();
            files.dedup();
            Ok(files)
        }
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictItem {
    pub path: String,
    pub branch: String,
    /// Files changed since branching off (committed or not).
    pub files: Vec<String>,
    /// Commits on the base the branch doesn't have yet: the base moved on since.
    pub behind: u32,
    /// Files that conflict if the committed work is merged into the base now.
    pub base_conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictPair {
    /// Worktree paths.
    pub a: String,
    pub b: String,
    /// Files both changed.
    pub overlap: Vec<String>,
    /// Of those, the ones whose committed changes actually conflict.
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictReport {
    pub items: Vec<ConflictItem>,
    /// Pairs of worktrees (of the same repo) that touch the same files.
    pub pairs: Vec<ConflictPair>,
}

/// Which worktrees would collide: with their base as it is now, and with each other once one
/// of them is merged. Worktrees that are gone are left out.
pub fn conflicts(infos: &[WorktreeInfo]) -> ConflictReport {
    let mut items: Vec<(WorktreeInfo, ConflictItem)> = Vec::new();
    for info in infos {
        let path = Path::new(&info.path);
        let Ok(files) = changed_files(info) else { continue };
        let behind = git(path, &["rev-list", "--count", &format!("HEAD..{}", info.base)]).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        // Run from the worktree, so HEAD is its branch's tip.
        let base_conflicts = merge_conflicts(path, &info.base, "HEAD").unwrap_or_default();
        items.push((info.clone(), ConflictItem { path: info.path.clone(), branch: info.branch.clone(), files, behind, base_conflicts }));
    }
    let mut pairs = Vec::new();
    for (i, (a, ia)) in items.iter().enumerate() {
        for (b, ib) in &items[i + 1..] {
            if a.repo != b.repo {
                continue;
            }
            let overlap: Vec<String> = ia.files.iter().filter(|f| ib.files.binary_search(f).is_ok()).cloned().collect();
            if overlap.is_empty() {
                continue;
            }
            let conflicts = merge_conflicts(Path::new(&a.path), "HEAD", &b.branch).unwrap_or_default();
            pairs.push(ConflictPair { a: a.path.clone(), b: b.path.clone(), overlap, conflicts });
        }
    }
    ConflictReport { items: items.into_iter().map(|(_, item)| item).collect(), pairs }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SyncOutcome {
    /// The branch already has everything on the base.
    UpToDate,
    /// The base was merged into the branch.
    Synced { commits: u32 },
    /// Merging the base in conflicts; nothing was changed.
    Conflicts { files: Vec<String> },
}

/// Brings the base's new commits into the worktree branch (a merge, so the agent's history stays
/// as it is), so tests can run on what the base will look like after merging. On conflicts it
/// backs out and names the files.
pub fn sync_with_base(info: &WorktreeInfo) -> Result<SyncOutcome, String> {
    let path = Path::new(&info.path);
    let behind: u32 = git(path, &["rev-list", "--count", &format!("HEAD..{}", info.base)])?.parse().unwrap_or(0);
    if behind == 0 {
        return Ok(SyncOutcome::UpToDate);
    }
    if !git(path, &["status", "--porcelain"])?.is_empty() {
        return Err("The worktree has uncommitted changes. Commit them first.".into());
    }
    let message = format!("Merge {} into {}", info.base, info.branch);
    match git(path, &["-c", "user.name=winfer", "-c", "user.email=winfer@localhost", "merge", "--no-edit", "-m", &message, &info.base]) {
        Ok(_) => Ok(SyncOutcome::Synced { commits: behind }),
        Err(err) => {
            let files: Vec<String> = git(path, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(String::from).collect();
            let _ = git(path, &["merge", "--abort"]);
            if files.is_empty() {
                Err(err)
            } else {
                Ok(SyncOutcome::Conflicts { files })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExistingWorktree {
    /// What a pane needs to work in it (diff, merge, review use `base`).
    pub info: WorktreeInfo,
    /// Created by winfer (`winfer/<provider>-<n>`), as opposed to by hand or another tool.
    pub by_winfer: bool,
    /// Its folder is gone (`git worktree prune` would drop it).
    pub prunable: bool,
    /// When the folder was created (ms since the epoch), if known.
    pub created_at: Option<u64>,
    /// When the branch was last committed to (ms since the epoch).
    pub last_commit_at: Option<u64>,
    /// Commits on the branch that aren't on the base.
    pub ahead: u32,
    /// Uncommitted changes in the folder.
    pub dirty: bool,
    /// Everything on the branch is already in the base: nothing is lost by removing it.
    pub merged: bool,
    /// The repo's main checkout (the project folder itself), listed first.
    #[serde(default)]
    pub is_main: bool,
}

/// The linked worktrees of the repo containing `dir` (not the main checkout itself).
/// Their base for diffs and merges is the main checkout's current branch.
pub fn list(dir: &Path) -> Result<Vec<ExistingWorktree>, String> {
    let Some(top) = repo_root(dir) else {
        return Ok(Vec::new());
    };
    let porcelain = git(&top, &["worktree", "list", "--porcelain"])?;
    // The main checkout is always listed first, even when `dir` is inside a linked worktree.
    let repo = porcelain
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .unwrap_or(top);
    let base = git(&repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .or_else(|_| git(&repo, &["rev-parse", "HEAD"]))?;
    let mut found = parse_worktree_list(&porcelain, &repo, &base);
    for wt in &mut found {
        describe(&repo, wt);
    }
    // The project folder itself comes first: agents can work there directly, without a worktree.
    let mut main = ExistingWorktree {
        info: WorktreeInfo {
            repo: repo.to_string_lossy().into_owned(),
            path: repo.to_string_lossy().into_owned(),
            branch: base.clone(),
            base: base.clone(),
        },
        by_winfer: false,
        prunable: false,
        created_at: None,
        last_commit_at: None,
        ahead: 0,
        dirty: false,
        merged: false,
        is_main: true,
    };
    main.last_commit_at = git(&repo, &["log", "-1", "--format=%ct"]).ok().and_then(|s| s.trim().parse::<u64>().ok()).map(|s| s * 1000);
    main.dirty = git(&repo, &["status", "--porcelain"]).is_ok_and(|s| !s.is_empty());
    found.insert(0, main);
    Ok(found)
}

/// Fills in age and state, for deciding whether a worktree can go.
fn describe(repo: &Path, wt: &mut ExistingWorktree) {
    let path = Path::new(&wt.info.path);
    let to_ms = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as u64);
    wt.created_at = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.created().or_else(|_| m.modified()).ok())
        .and_then(to_ms);
    if wt.prunable {
        return;
    }
    let branch = &wt.info.branch;
    wt.last_commit_at = git(repo, &["log", "-1", "--format=%ct", branch])
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|secs| secs * 1000);
    wt.ahead = git(repo, &["rev-list", "--count", &format!("{}..{branch}", wt.info.base)])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    wt.dirty = git(path, &["status", "--porcelain"]).is_ok_and(|s| !s.is_empty());
    wt.merged = wt.ahead == 0 && !wt.dirty;
}

fn parse_worktree_list(porcelain: &str, repo: &Path, base: &str) -> Vec<ExistingWorktree> {
    let same = |a: &Path, b: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(b)
    };
    porcelain
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;
            let mut detached_head = None;
            let mut prunable = false;
            let mut bare = false;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(p));
                } else if let Some(b) = line.strip_prefix("branch ") {
                    branch = Some(b.trim_start_matches("refs/heads/").to_string());
                } else if let Some(h) = line.strip_prefix("HEAD ") {
                    detached_head = Some(h.chars().take(8).collect::<String>());
                } else if line.starts_with("prunable") {
                    prunable = true;
                } else if line == "bare" {
                    bare = true;
                }
            }
            let path = path?;
            if bare || same(&path, repo) {
                return None;
            }
            let branch = branch.or(detached_head)?;
            Some(ExistingWorktree {
                by_winfer: branch.starts_with("winfer/"),
                prunable,
                created_at: None,
                last_commit_at: None,
                ahead: 0,
                dirty: false,
                merged: false,
                is_main: false,
                info: WorktreeInfo {
                    repo: repo.to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                    branch,
                    base: base.to_string(),
                },
            })
        })
        .collect()
}

/// Commits everything the agent left uncommitted in its worktree (no-op when clean), so the
/// branch can be merged.
pub fn commit_all(info: &WorktreeInfo, message: &str) -> Result<(), String> {
    let path = Path::new(&info.path);
    if git(path, &["status", "--porcelain"])?.is_empty() {
        return Ok(());
    }
    git(path, &["add", "-A"])?;
    git(path, &["commit", "--no-verify", "-m", message]).map(|_| ())
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
pub async fn worktree_list(dir: String) -> Result<Vec<ExistingWorktree>, String> {
    list(&resolve_cwd(Some(&dir)))
}

#[tauri::command]
pub async fn worktree_branches(dir: String) -> Result<Vec<Branch>, String> {
    branches(&resolve_cwd(Some(&dir)))
}

#[tauri::command]
pub async fn worktree_from_branch(app: AppHandle, dir: String, branch: String, new_branch: Option<String>) -> Result<WorktreeInfo, String> {
    create_from_branch(&worktrees_dir(&app)?, &resolve_cwd(Some(&dir)), &branch, new_branch.as_deref())
}

#[tauri::command]
pub async fn worktree_fetch(dir: String) -> Result<(), String> {
    fetch(&resolve_cwd(Some(&dir)))
}

#[tauri::command]
pub async fn worktree_commit(info: WorktreeInfo, message: String) -> Result<(), String> {
    commit_all(&info, &message)
}

#[tauri::command]
pub async fn worktree_conflicts(infos: Vec<WorktreeInfo>) -> Result<ConflictReport, String> {
    Ok(conflicts(&infos))
}

#[tauri::command]
pub async fn worktree_sync(info: WorktreeInfo) -> Result<SyncOutcome, String> {
    sync_with_base(&info)
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
    fn new_worktrees_get_a_copy_of_the_codegraph_index() {
        let plain = repo();
        let indexed = repo();
        std::fs::create_dir_all(indexed.join(".codegraph")).unwrap();
        std::fs::write(indexed.join(".codegraph/codegraph.db"), "db").unwrap();
        std::fs::write(indexed.join(".codegraph/daemon.pid"), "1").unwrap();
        let wt = create(&temp_dir("wt"), &indexed, "claude").unwrap().unwrap();
        let index = Path::new(&wt.path).join(".codegraph");
        assert_eq!(std::fs::read_to_string(index.join("codegraph.db")).unwrap(), "db");
        assert!(!index.join("daemon.pid").exists());
        // No index in the repo: none in the worktree.
        let wt = create(&temp_dir("wt"), &plain, "claude").unwrap().unwrap();
        assert!(!Path::new(&wt.path).join(".codegraph").exists());
    }

    #[test]
    fn non_repo_returns_none() {
        assert_eq!(
            create(&temp_dir("wt"), &temp_dir("plain"), "claude").unwrap(),
            None
        );
    }

    #[test]
    fn concurrent_creates_get_distinct_worktrees() {
        // A/B tests start both agents at once with the same provider.
        let repo = repo();
        let wts = temp_dir("wt");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (repo, wts) = (repo.clone(), wts.clone());
                std::thread::spawn(move || create(&wts, &repo, "claude").unwrap().unwrap())
            })
            .collect();
        let mut branches: Vec<_> = handles.into_iter().map(|h| h.join().unwrap().branch).collect();
        branches.sort();
        assert_eq!(
            branches,
            ["winfer/claude-1", "winfer/claude-2", "winfer/claude-3", "winfer/claude-4"]
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
    fn lists_the_main_checkout_first_then_worktrees() {
        let repo = repo();
        let wts = temp_dir("wt");
        let ours = create(&wts, &repo, "codex").unwrap().unwrap();
        // One made by hand, outside winfer.
        let manual = wts.join("hotfix");
        git(&repo, &["worktree", "add", "-b", "hotfix", &manual.to_string_lossy()]).unwrap();

        let mut found = list(&repo).unwrap();
        // The project folder itself comes first, on its own branch.
        let main = found.remove(0);
        assert!(main.is_main && main.info.branch == "main", "{main:?}");
        assert_eq!(std::fs::canonicalize(&main.info.path).unwrap(), std::fs::canonicalize(&repo).unwrap());
        assert!(found.iter().all(|w| !w.is_main));
        found.sort_by(|a, b| a.info.branch.cmp(&b.info.branch));
        let branches: Vec<_> = found.iter().map(|w| (w.info.branch.as_str(), w.by_winfer)).collect();
        assert_eq!(branches, vec![("hotfix", false), ("winfer/codex-1", true)]);
        assert!(found.iter().all(|w| w.info.base == "main" && !w.prunable));
        // Fresh worktrees have nothing new yet, so they count as merged (safe to remove)…
        assert!(found.iter().all(|w| w.merged && w.ahead == 0 && w.last_commit_at.is_some() && w.created_at.is_some()));
        // …until they get work: uncommitted first, then a commit the base doesn't have.
        sh(Path::new(&ours.path), "echo more >> a.txt");
        let state = |l: Vec<ExistingWorktree>| l.into_iter().find(|w| w.info.branch == ours.branch && !w.is_main).map(|w| (w.dirty, w.ahead, w.merged)).unwrap();
        assert_eq!(state(list(&repo).unwrap()), (true, 0, false));
        sh(Path::new(&ours.path), "git -c user.email=t@t -c user.name=t commit -qam work");
        assert_eq!(state(list(&repo).unwrap()), (false, 1, false));
        // Works from inside a worktree too, and hands back what create() made.
        let from_inside = list(Path::new(&ours.path)).unwrap();
        assert!(from_inside.iter().any(|w| w.info.branch == ours.branch));

        remove(&ours, true).unwrap();
        assert!(list(&repo).unwrap().iter().all(|w| w.info.branch != ours.branch));
    }

    #[test]
    fn worktrees_from_local_new_and_remote_branches() {
        let origin = repo();
        sh(&origin, "git branch feature/login && git branch release");
        // A clone, so the origin's branches show up as remote ones.
        let clone = temp_dir("clone");
        // Quoted with forward slashes: sh would eat a Windows path's backslashes.
        let origin_path = origin.display().to_string().replace('\\', "/");
        sh(&clone, &format!("git clone -q '{origin_path}' . && git config user.email t@t && git config user.name t"));
        sh(&clone, "git branch local-only");
        let wts = temp_dir("wt");

        let names: Vec<_> = branches(&clone).unwrap().into_iter().map(|b| (b.name, b.remote)).collect();
        assert!(names.contains(&("main".into(), false)) && names.contains(&("local-only".into(), false)));
        assert!(names.contains(&("origin/feature/login".into(), true)));
        // origin/main has a local twin, so it isn't offered twice.
        assert!(!names.iter().any(|(n, _)| n == "origin/main"));

        // Remote branch: a local tracking branch with the short name.
        let login = create_from_branch(&wts, &clone, "origin/feature/login", None).unwrap();
        assert_eq!((login.branch.as_str(), login.base.as_str()), ("feature/login", "main"));
        assert!(git(Path::new(&login.path), &["rev-parse", "--abbrev-ref", "@{u}"]).unwrap().contains("origin/feature/login"));

        // Existing local branch, and a new branch started from one.
        let local = create_from_branch(&wts, &clone, "local-only", None).unwrap();
        assert_eq!(local.branch, "local-only");
        let fresh = create_from_branch(&wts, &clone, "main", Some("spike/idea")).unwrap();
        assert_eq!(fresh.branch, "spike/idea");
        assert!(fresh.path.ends_with("spike-idea"));

        // The checked-out branch (main) can't get a second worktree; a clear error instead.
        assert!(create_from_branch(&wts, &clone, "main", None).unwrap_err().contains("already checked out"));
        assert!(create_from_branch(&wts, &clone, "main", Some("bad name")).unwrap_err().contains("valid branch name"));
        assert!(create_from_branch(&wts, &clone, "main", Some("local-only")).unwrap_err().contains("already exists"));

        // Branches now show where they're checked out; works from inside a worktree too.
        let from_inside = branches(Path::new(&local.path)).unwrap();
        let local_only = from_inside.iter().find(|b| b.name == "local-only").unwrap();
        assert!(local_only.checked_out_at.as_deref().is_some_and(|p| p.ends_with("local-only")));
    }

    #[test]
    fn previews_conflicts_with_the_base_and_between_worktrees() {
        let repo = repo();
        sh(&repo, "printf 'a\\nb\\nc\\n' > lines.txt && git add . && git commit -qm lines");
        let wts = temp_dir("wt");
        let commit = "git -c user.email=t@t -c user.name=t commit -qam";
        let a = create(&wts, &repo, "claude").unwrap().unwrap();
        let b = create(&wts, &repo, "codex").unwrap().unwrap();
        let c = create(&wts, &repo, "opencode").unwrap().unwrap();
        // a and b edit the same line; c edits another file, uncommitted.
        sh(Path::new(&a.path), &format!("sed -i.bak 's/^b$/from-a/' lines.txt && rm lines.txt.bak && {commit} a"));
        sh(Path::new(&b.path), &format!("sed -i.bak 's/^b$/from-b/' lines.txt && rm lines.txt.bak && {commit} b"));
        sh(Path::new(&c.path), "echo c >> a.txt");

        let r = conflicts(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!(r.items.len(), 3);
        assert!(r.items.iter().all(|i| i.behind == 0 && i.base_conflicts.is_empty()), "{r:?}");
        assert_eq!(r.items[2].files, vec!["a.txt"]);
        assert_eq!(r.pairs, vec![ConflictPair { a: a.path.clone(), b: b.path.clone(), overlap: vec!["lines.txt".into()], conflicts: vec!["lines.txt".into()] }]);

        // Once a is merged, b conflicts with the base and can't be synced; c syncs cleanly.
        merge(&a).unwrap();
        let r = conflicts(&[b.clone(), c.clone()]);
        assert_eq!((r.items[0].behind, r.items[0].base_conflicts.clone()), (2, vec!["lines.txt".to_string()]));
        assert_eq!(sync_with_base(&b).unwrap(), SyncOutcome::Conflicts { files: vec!["lines.txt".into()] });
        sh(Path::new(&b.path), "test -z \"$(git status --porcelain)\" && grep -q from-b lines.txt");
        assert!(sync_with_base(&c).unwrap_err().contains("uncommitted"));
        sh(Path::new(&c.path), &format!("{commit} c"));
        assert_eq!(sync_with_base(&c).unwrap(), SyncOutcome::Synced { commits: 2 });
        assert_eq!(sync_with_base(&c).unwrap(), SyncOutcome::UpToDate);
        sh(Path::new(&c.path), "grep -q from-a lines.txt");
        merge(&c).unwrap();
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
