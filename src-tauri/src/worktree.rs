//! One git worktree per agent pane, so parallel agents never edit the same checkout.
//!
//! Worktrees live under the app data dir (`worktrees/<repo>-<hash>/<provider>-<n>`) on branches
//! named `winfer/<provider>-<n>`, created from the repo's current branch.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// Longest a git command may run. One stuck on an antivirus scan, a network drive or a
/// credential prompt must not hold its thread (and the UI waiting on it) forever.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Fetching talks to remotes, which can be slow on big repos.
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);

struct GitOutput {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs git in `dir`, killing it after `timeout`.
///
/// `GIT_OPTIONAL_LOCKS=0`: the sidebar polls `git status`, which would otherwise refresh the
/// index and take `index.lock`, making an agent's own git commands in that worktree fail with
/// "Another git process seems to be running".
fn run_git(dir: &Path, args: &[&str], timeout: Duration) -> Result<GitOutput, String> {
    let mut child = crate::env::no_window(&mut Command::new("git"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(login_env())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    // Most git calls take a few ms: poll often at first, then back off.
    let mut pause = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // The drain threads end once git's helpers let go of the pipes; not waited for.
                return Err(format!("git {} took longer than {}s and was stopped", args.first().unwrap_or(&""), timeout.as_secs()));
            }
            Ok(None) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("couldn't run git: {e}")),
        }
    };
    Ok(GitOutput {
        code: status.code(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Reads a child's pipe to the end on its own thread, so a full pipe can't stall the child
/// while we wait for it.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

fn git_with_timeout(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = run_git(dir, args, timeout)?;
    if out.code == Some(0) {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_with_timeout(dir, args, GIT_TIMEOUT)
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
    /// The remote branch it can be brought up to date from ("origin/feature/login"): its
    /// upstream for a local branch, itself for a remote one.
    pub upstream: Option<String>,
}

/// Local branches (newest first), then remote ones without a local branch of the same name.
pub fn branches(dir: &Path) -> Result<Vec<Branch>, String> {
    let repo = main_checkout(dir).ok_or("not a git repository")?;
    let refs = git(
        &repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%09%(committerdate:unix)%09%(worktreepath)%09%(upstream:short)%09%(upstream:remotename)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in refs.lines() {
        let mut parts = line.split('\t');
        let (Some(refname), Some(date), worktree, upstream, upstream_remote) =
            (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let last_commit_at = date.trim().parse::<u64>().ok().map(|s| s * 1000);
        let checked_out_at = worktree.filter(|w| !w.is_empty()).map(String::from);
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            // An upstream that's another local branch (remote ".") has nothing to pull.
            let upstream = upstream.filter(|u| !u.is_empty() && upstream_remote.is_some_and(|r| !r.is_empty() && r != ".")).map(String::from);
            local.push(Branch { name: name.into(), remote: false, last_commit_at, checked_out_at, upstream });
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if !name.ends_with("/HEAD") {
                remote.push(Branch { name: name.into(), remote: true, last_commit_at, checked_out_at: None, upstream: Some(name.into()) });
            }
        }
    }
    // A remote branch that already has a local twin is reached through the local one.
    let locals: std::collections::HashSet<String> = local.iter().map(|b| b.name.clone()).collect();
    remote.retain(|b| !b.name.split_once('/').is_some_and(|(_, short)| locals.contains(short)));
    local.extend(remote);
    Ok(local)
}

fn already_checked_out(branch: &str) -> String {
    format!("“{branch}” is already checked out in another folder. Open that worktree from the sidebar instead.")
}

/// Fetches the remote `branch` comes from and returns what to start the worktree from so it has
/// the remote's latest commits. A local branch behind its upstream is fast-forwarded (a new
/// branch starts from the upstream instead); one with unpushed commits on top is already up to
/// date; one that has diverged is an error.
fn pull_latest(repo: &Path, branch: &str, is_remote: bool, new_branch: bool) -> Result<String, String> {
    let (upstream, remote) = if is_remote {
        // "origin/x": the remote is the longest remote name the ref starts with.
        let remotes = git(repo, &["remote"])?;
        let remote = remotes
            .lines()
            .filter(|r| branch.strip_prefix(r).is_some_and(|rest| rest.starts_with('/')))
            .max_by_key(|r| r.len())
            .ok_or_else(|| format!("Couldn't tell which remote “{branch}” comes from."))?
            .to_string();
        (branch.to_string(), remote)
    } else {
        let local_ref = format!("refs/heads/{branch}");
        let up = git(repo, &["for-each-ref", "--format=%(upstream:short)%09%(upstream:remotename)", &local_ref])?;
        match up.split_once('\t') {
            Some((u, r)) if !u.is_empty() && !r.is_empty() && r != "." => (u.to_string(), r.to_string()),
            _ => return Err(format!("“{branch}” has no remote branch to pull from. Use the local branch instead.")),
        }
    };
    git_with_timeout(repo, &["fetch", "--quiet", "--prune", &remote], FETCH_TIMEOUT).map_err(|e| format!("Fetching {remote} failed: {e}"))?;
    let latest = git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{upstream}^{{commit}}")])
        .map_err(|_| format!("“{upstream}” is gone from {remote}."))?;
    if is_remote {
        return Ok(branch.to_string());
    }

    let current = git(repo, &["rev-parse", &format!("refs/heads/{branch}")])?;
    let is_ancestor = |a: &str, b: &str| git(repo, &["merge-base", "--is-ancestor", a, b]).is_ok();
    if current == latest || is_ancestor(&latest, &current) {
        // Up to date, or ahead with unpushed commits.
        return Ok(branch.to_string());
    }
    if !is_ancestor(&current, &latest) {
        return Err(format!("“{branch}” and “{upstream}” have diverged. Pull it by hand, or use the local branch."));
    }
    // Behind. A new branch just starts from the upstream, leaving the local one alone.
    if new_branch {
        return Ok(upstream);
    }
    // Otherwise fast-forward it, unless it's checked out (moving the ref under its files would
    // make them look changed; the worktree couldn't be made anyway).
    let checked_out = !git(repo, &["for-each-ref", "--format=%(worktreepath)", &format!("refs/heads/{branch}")])?.is_empty();
    if checked_out {
        return Err(already_checked_out(branch));
    }
    git(repo, &["update-ref", "-m", "winfer: fast-forward to remote", &format!("refs/heads/{branch}"), &latest, &current])?;
    Ok(branch.to_string())
}

/// A worktree for an existing branch, or — with `new_branch` — for a new branch started from it.
/// Remote branches ("origin/x") get a local tracking branch "x". With `latest`, the branch is
/// first brought up to date with its remote (see `pull_latest`); otherwise it's used as it is.
pub fn create_from_branch(
    worktrees_dir: &Path,
    dir: &Path,
    branch: &str,
    new_branch: Option<&str>,
    latest: bool,
) -> Result<WorktreeInfo, String> {
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
    let start = if latest { pull_latest(&repo, branch, is_remote, new_branch.is_some())? } else { branch.to_string() };
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
        // --no-track: started from a remote branch, it would otherwise track one of another name.
        (Some(name), _) => git(&repo, &["worktree", "add", "--no-track", "-b", name, &path_str, &start]),
        (None, true) => git(&repo, &["worktree", "add", "--track", "-b", &local, &path_str, &start]),
        (None, false) => git(&repo, &["worktree", "add", &path_str, &start]),
    }
    .map_err(|e| {
        if e.contains("already checked out") || e.contains("is already used by worktree") {
            already_checked_out(branch)
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
    git_with_timeout(&repo, &["fetch", "--all", "--prune", "--quiet"], FETCH_TIMEOUT).map(|_| ())
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
    let out = run_git(repo, &["merge-tree", "--write-tree", "--name-only", "--no-messages", ours, theirs], GIT_TIMEOUT)?;
    match out.code {
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveRequest {
    pub info: WorktreeInfo,
    pub delete_branch: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoveFailure {
    pub path: String,
    pub branch: String,
    pub error: String,
}

/// Removes several worktrees one after another (git locks its admin files, so not in parallel).
/// One failing doesn't stop the rest; the failures come back.
pub fn remove_many(items: &[RemoveRequest]) -> Vec<RemoveFailure> {
    items
        .iter()
        .filter_map(|item| {
            remove(&item.info, item.delete_branch).err().map(|error| RemoveFailure {
                path: item.info.path.clone(),
                branch: item.info.branch.clone(),
                error,
            })
        })
        .collect()
}

fn worktrees_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("worktrees"))
}

/// Runs git work on a blocking thread. Async commands share a few runtime threads with the rest
/// of the app; git calls that take seconds (big repos, Windows process start-up, antivirus)
/// would starve every other command and freeze the UI.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn worktree_create(
    app: AppHandle,
    dir: String,
    provider: String,
) -> Result<Option<WorktreeInfo>, String> {
    let worktrees = worktrees_dir(&app)?;
    blocking(move || create(&worktrees, &resolve_cwd(Some(&dir)), &provider)).await
}

#[tauri::command]
pub async fn worktree_diff(info: WorktreeInfo) -> Result<WorktreeDiff, String> {
    blocking(move || diff(&info)).await
}

/// Just the names of the files a worktree changed: for callers that don't need the diff itself.
#[tauri::command]
pub async fn worktree_changed_files(info: WorktreeInfo) -> Result<Vec<String>, String> {
    blocking(move || changed_files(&info)).await
}

#[tauri::command]
pub async fn worktree_merge(info: WorktreeInfo) -> Result<String, String> {
    blocking(move || merge(&info)).await
}

#[tauri::command]
pub async fn worktree_list(dir: String) -> Result<Vec<ExistingWorktree>, String> {
    blocking(move || list(&resolve_cwd(Some(&dir)))).await
}

#[tauri::command]
pub async fn worktree_branches(dir: String) -> Result<Vec<Branch>, String> {
    blocking(move || branches(&resolve_cwd(Some(&dir)))).await
}

#[tauri::command]
pub async fn worktree_from_branch(
    app: AppHandle,
    dir: String,
    branch: String,
    new_branch: Option<String>,
    latest: Option<bool>,
) -> Result<WorktreeInfo, String> {
    let worktrees = worktrees_dir(&app)?;
    let latest = latest.unwrap_or(false);
    blocking(move || create_from_branch(&worktrees, &resolve_cwd(Some(&dir)), &branch, new_branch.as_deref(), latest)).await
}

#[tauri::command]
pub async fn worktree_fetch(dir: String) -> Result<(), String> {
    blocking(move || fetch(&resolve_cwd(Some(&dir)))).await
}

#[tauri::command]
pub async fn worktree_commit(info: WorktreeInfo, message: String) -> Result<(), String> {
    blocking(move || commit_all(&info, &message)).await
}

#[tauri::command]
pub async fn worktree_conflicts(infos: Vec<WorktreeInfo>) -> Result<ConflictReport, String> {
    blocking(move || Ok(conflicts(&infos))).await
}

#[tauri::command]
pub async fn worktree_sync(info: WorktreeInfo) -> Result<SyncOutcome, String> {
    blocking(move || sync_with_base(&info)).await
}

#[tauri::command]
pub async fn worktree_remove(info: WorktreeInfo, delete_branch: bool) -> Result<(), String> {
    blocking(move || remove(&info, delete_branch)).await
}

#[tauri::command]
pub async fn worktree_remove_many(items: Vec<RemoveRequest>) -> Result<Vec<RemoveFailure>, String> {
    blocking(move || Ok(remove_many(&items))).await
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
    fn removes_many_and_reports_the_ones_that_fail() {
        let repo = repo();
        let wts = temp_dir("wt");
        let a = create(&wts, &repo, "claude").unwrap().unwrap();
        let b = create(&wts, &repo, "claude").unwrap().unwrap();
        let gone = WorktreeInfo { path: wts.join("nope").to_string_lossy().into_owned(), branch: "winfer/nope".into(), ..a.clone() };
        let failed = remove_many(&[
            RemoveRequest { info: a.clone(), delete_branch: true },
            RemoveRequest { info: gone.clone(), delete_branch: true },
            RemoveRequest { info: b.clone(), delete_branch: false },
        ]);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].path, gone.path);
        assert!(!Path::new(&a.path).exists() && !Path::new(&b.path).exists());
        assert!(!branch_exists(&repo, &a.branch));
        // Only the branches asked for go.
        assert!(branch_exists(&repo, &b.branch));
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
        let login = create_from_branch(&wts, &clone, "origin/feature/login", None, false).unwrap();
        assert_eq!((login.branch.as_str(), login.base.as_str()), ("feature/login", "main"));
        assert!(git(Path::new(&login.path), &["rev-parse", "--abbrev-ref", "@{u}"]).unwrap().contains("origin/feature/login"));

        // Existing local branch, and a new branch started from one.
        let local = create_from_branch(&wts, &clone, "local-only", None, false).unwrap();
        assert_eq!(local.branch, "local-only");
        let fresh = create_from_branch(&wts, &clone, "main", Some("spike/idea"), false).unwrap();
        assert_eq!(fresh.branch, "spike/idea");
        assert!(fresh.path.ends_with("spike-idea"));

        // The checked-out branch (main) can't get a second worktree; a clear error instead.
        assert!(create_from_branch(&wts, &clone, "main", None, false).unwrap_err().contains("already checked out"));
        assert!(create_from_branch(&wts, &clone, "main", Some("bad name"), false).unwrap_err().contains("valid branch name"));
        assert!(create_from_branch(&wts, &clone, "main", Some("local-only"), false).unwrap_err().contains("already exists"));

        // Branches now show where they're checked out; works from inside a worktree too.
        let from_inside = branches(Path::new(&local.path)).unwrap();
        let local_only = from_inside.iter().find(|b| b.name == "local-only").unwrap();
        assert!(local_only.checked_out_at.as_deref().is_some_and(|p| p.ends_with("local-only")));
    }

    #[test]
    fn worktrees_from_the_latest_remote_branch() {
        let origin = repo();
        sh(&origin, "git branch release && git branch feature/login && git branch wip");
        let clone = temp_dir("clone");
        let origin_path = origin.display().to_string().replace('\\', "/");
        sh(&clone, &format!("git clone -q '{origin_path}' . && git config user.email t@t && git config user.name t"));
        sh(&clone, "git branch release origin/release && git branch wip origin/wip && git branch local-only");
        // The remote moves on after the clone; wip also gets a local commit, so it diverges.
        sh(&origin, "for b in release feature/login wip; do git checkout -q $b && echo $b >> a.txt && git commit -qam $b; done && git checkout -q main");
        sh(&clone, "git checkout -q wip && echo mine >> a.txt && git commit -qam mine && git checkout -q main");
        let wts = temp_dir("wt");
        let head = |p: &str| git(Path::new(p), &["rev-parse", "HEAD"]).unwrap();
        let tip = |b: &str| git(&origin, &["rev-parse", b]).unwrap();

        let ups: Vec<_> = branches(&clone).unwrap().into_iter().map(|b| (b.name, b.upstream)).collect();
        assert!(ups.contains(&("release".into(), Some("origin/release".into()))));
        assert!(ups.contains(&("local-only".into(), None)));

        // As it is locally: the old commit. Latest: the remote's, fetched first.
        let stale = create_from_branch(&wts, &clone, "release", Some("r-old"), false).unwrap();
        assert_ne!(head(&stale.path), tip("release"));
        let fresh = create_from_branch(&wts, &clone, "release", Some("r-new"), true).unwrap();
        assert_eq!(head(&fresh.path), tip("release"));
        // A new branch doesn't track the remote one it started from.
        assert!(git(Path::new(&fresh.path), &["rev-parse", "--abbrev-ref", "@{u}"]).is_err());
        // Working on the branch itself fast-forwards it.
        let release = create_from_branch(&wts, &clone, "release", None, true).unwrap();
        assert_eq!(head(&release.path), tip("release"));
        // Remote branches too.
        let login = create_from_branch(&wts, &clone, "origin/feature/login", None, true).unwrap();
        assert_eq!(head(&login.path), tip("feature/login"));

        assert!(create_from_branch(&wts, &clone, "local-only", None, true).unwrap_err().contains("no remote branch"));
        assert!(create_from_branch(&wts, &clone, "wip", None, true).unwrap_err().contains("diverged"));
        // The checked-out branch isn't moved under its files.
        sh(&origin, "echo more >> a.txt && git commit -qam more");
        let main_before = git(&clone, &["rev-parse", "main"]).unwrap();
        assert!(create_from_branch(&wts, &clone, "main", None, true).unwrap_err().contains("already checked out"));
        assert_eq!(git(&clone, &["rev-parse", "main"]).unwrap(), main_before);
        // …but a new branch can still start from the remote's latest.
        let spike = create_from_branch(&wts, &clone, "main", Some("spike"), true).unwrap();
        assert_eq!(head(&spike.path), tip("main"));
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
