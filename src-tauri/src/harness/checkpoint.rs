//! Workspace checkpoints for /undo: before and after each turn, the working tree is snapshotted
//! into a shadow Git repository (in winfer's data dir, never the project's own `.git`), so undoing
//! a turn restores exactly what it changed, however it changed it: edit tools, files created or
//! deleted by shell commands, subagents. Files the project ignores (`.gitignore`) aren't tracked.
//!
//! Snapshots are trees written from a per-session index, so they're fast after the first one
//! (Git only rehashes files whose stat changed) and sessions in the same folder don't contend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};

use sha2::{Digest, Sha256};

/// Heavy folders skipped even where the project doesn't ignore them (or isn't a Git repo).
const EXCLUDES: &str = "node_modules/\ntarget/\n.venv/\nvenv/\n__pycache__/\n.next/\n.nuxt/\n.gradle/\n.DS_Store\n";

pub struct Checkpoints {
    git_dir: PathBuf,
    /// The snapshotted folder: the enclosing Git repository's root, else the working directory.
    root: PathBuf,
    index: PathBuf,
}

impl Checkpoints {
    /// Checkpoints for `cwd`, kept in `store`. None without Git, or for folders too broad to
    /// snapshot (the home folder, a filesystem root).
    pub fn open(cwd: &Path, store: &Path, session: &str) -> Option<Self> {
        crate::env::find_command("git")?;
        let cwd = cwd.canonicalize().ok()?;
        let root = git_root(&cwd).unwrap_or_else(|| cwd.clone());
        if root.parent().is_none() || dirs::home_dir().and_then(|h| h.canonicalize().ok()).is_some_and(|h| h == root) {
            return None;
        }
        let key = hex(&Sha256::digest(root.to_string_lossy().as_bytes())[..8]);
        let git_dir = store.join(key);
        if !git_dir.join("HEAD").exists() {
            std::fs::create_dir_all(&git_dir).ok()?;
            let ok = Command::new("git").args(["init", "--bare", "-q"]).arg(&git_dir).stdout(Stdio::null()).stderr(Stdio::null()).status().ok()?.success();
            if !ok {
                return None;
            }
        }
        let _ = std::fs::create_dir_all(git_dir.join("info"));
        let _ = std::fs::write(git_dir.join("info").join("exclude"), EXCLUDES);
        let _ = std::fs::write(git_dir.join("winfer-root"), root.to_string_lossy().as_bytes());
        let index = git_dir.join(format!("index-{session}"));
        // Start from the last session's index so the first snapshot doesn't rehash everything.
        if !index.exists() {
            let _ = std::fs::copy(git_dir.join("index"), &index);
        }
        Some(Checkpoints { git_dir, root, index })
    }

    fn git(&self, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let mut cmd = Command::new("git");
        cmd.arg("--git-dir").arg(&self.git_dir).arg("--work-tree").arg(&self.root);
        cmd.args(["-c", "core.autocrlf=false", "-c", "core.safecrlf=false", "-c", "core.fsmonitor=false", "-c", "core.quotepath=false", "-c", "gc.auto=0", "-c", "core.longpaths=true"]);
        cmd.arg("--literal-pathspecs").args(args).current_dir(&self.root).env("GIT_INDEX_FILE", &self.index);
        for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE"] {
            cmd.env_remove(var);
        }
        cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        crate::env::no_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("couldn't run git: {e}"))?;
        if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
            use std::io::Write;
            stdin.write_all(data).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed").trim().to_string());
        }
        Ok(out.stdout)
    }

    /// Snapshots the working tree; returns its tree id.
    pub fn snapshot(&self) -> Result<String, String> {
        let tree = self.write_tree().or_else(|_| {
            // An index naming objects pruned since: start it over (everything is rehashed).
            let _ = std::fs::remove_file(&self.index);
            self.write_tree()
        })?;
        // Next session's starting point (best effort; another session may be writing it too).
        let tmp = self.git_dir.join(format!("index.{}.tmp", std::process::id()));
        if std::fs::copy(&self.index, &tmp).is_ok() {
            let _ = std::fs::rename(&tmp, self.git_dir.join("index"));
        }
        Ok(tree)
    }

    fn write_tree(&self) -> Result<String, String> {
        self.git(&["add", "-A", "--ignore-errors", "."], None).or_else(|e| {
            // Unreadable files are skipped, but `add` still exits 1 for them.
            if e.contains("unable to") || e.contains("Permission denied") { Ok(Vec::new()) } else { Err(e) }
        })?;
        Ok(String::from_utf8_lossy(&self.git(&["write-tree"], None)?).trim().to_string())
    }

    /// Puts back what changed between the snapshots `before` and `after`: files created are
    /// removed, deleted and modified ones restored as they were in `before`. Changes made since
    /// `after` to other files stay. Returns the paths it put back, relative to the root.
    pub fn restore(&self, before: &str, after: &str) -> Result<Vec<String>, String> {
        let out = self.git(&["diff-tree", "-r", "-z", "--no-renames", "--name-status", before, after], None)?;
        let mut fields = out.split(|&b| b == 0).filter(|f| !f.is_empty());
        let mut created = Vec::new();
        let mut restored = Vec::new();
        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            let path = String::from_utf8_lossy(path).into_owned();
            match status.first() {
                Some(b'A') => created.push(path),
                _ => restored.push(path),
            }
        }
        for path in &created {
            let full = self.root.join(path);
            if std::fs::remove_file(&full).is_ok() {
                // Folders the turn made and left empty go too.
                let mut dir = full.parent();
                while let Some(d) = dir.filter(|d| *d != self.root && d.starts_with(&self.root)) {
                    if std::fs::remove_dir(d).is_err() {
                        break;
                    }
                    dir = d.parent();
                }
            }
        }
        if !restored.is_empty() {
            // A file the turn replaced with a folder (or the other way round) can't be checked out over.
            for path in &restored {
                let full = self.root.join(path);
                if full.is_dir() {
                    let _ = std::fs::remove_dir_all(&full);
                }
            }
            let list: Vec<u8> = restored.iter().flat_map(|p| p.bytes().chain([0])).collect();
            self.git(&["checkout", before, "--pathspec-from-file=-", "--pathspec-file-nul"], Some(&list))?;
        }
        // The index matches the restored tree, so the next snapshot only rehashes what changed.
        let _ = self.git(&["add", "-A", "--ignore-errors", "."], None);
        let mut all: Vec<String> = created.into_iter().chain(restored).collect();
        all.sort();
        Ok(all)
    }

    /// Drops what old sessions left: their index files, and (weekly) snapshots older than a week,
    /// which nothing references.
    fn tidy(&self) {
        let old = |p: &Path, days: u64| {
            std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > days * 86_400)
        };
        for entry in std::fs::read_dir(&self.git_dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with("index-") || name.ends_with(".tmp")) && path != self.index && old(&path, 3) {
                let _ = std::fs::remove_file(&path);
            }
        }
        let stamp = self.git_dir.join("winfer-pruned");
        if !stamp.exists() || old(&stamp, 7) {
            let _ = std::fs::write(&stamp, b"");
            let _ = self.git(&["prune", "--expire=7.days.ago"], None);
            // It may name pruned objects; sessions starting from it would trip over them.
            let _ = std::fs::remove_file(self.git_dir.join("index"));
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "--show-toplevel"]).current_dir(cwd).stderr(Stdio::null());
    crate::env::no_window(&mut cmd);
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    path.canonicalize().ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where checkpoints live.
pub fn store() -> PathBuf {
    super::config::dir().join("checkpoints")
}

/// What the TUI asks of the checkpoint thread, by turn id. Jobs run in order, so a turn's
/// snapshots are always taken before it's restored.
pub enum Job {
    /// Snapshot before the turn starts; answered with [`Done::Began`].
    Begin(u64),
    /// Snapshot after the turn ended.
    End(u64),
    /// Undo the turn's changes; answered with [`Done::Restored`].
    Restore(u64),
    /// Forget all turns (new session).
    Clear,
}

pub enum Done {
    /// The turn may start; whether it has a snapshot to go back to.
    Began(u64, bool),
    /// The paths the turn's undo put back, relative to the working directory where possible.
    Restored(Result<Vec<String>, String>),
}

/// A thread taking snapshots off the UI thread.
pub struct Worker {
    tx: Sender<Job>,
}

impl Worker {
    /// Starts the thread for `cwd`; its answers go through `reply`. Without usable checkpoints
    /// it still answers (`Began(id, false)`), so the caller needn't care.
    pub fn start(cwd: PathBuf, store: PathBuf, session: String, reply: impl Fn(Done) + Send + 'static) -> Worker {
        let (tx, rx) = channel::<Job>();
        std::thread::spawn(move || {
            let cp = Checkpoints::open(&cwd, &store, &session);
            // Warm the index while the user types the first message.
            if let Some(cp) = &cp {
                let _ = cp.snapshot();
                cp.tidy();
            }
            let base = cwd.canonicalize().unwrap_or(cwd);
            let mut turns: HashMap<u64, (String, Option<String>)> = HashMap::new();
            for job in rx {
                match job {
                    Job::Begin(id) => {
                        let tree = cp.as_ref().and_then(|c| c.snapshot().ok());
                        let ok = tree.is_some();
                        if let Some(t) = tree {
                            turns.insert(id, (t, None));
                        }
                        reply(Done::Began(id, ok));
                    }
                    Job::End(id) => {
                        if let (Some(cp), Some(turn)) = (&cp, turns.get_mut(&id)) {
                            turn.1 = cp.snapshot().ok();
                        }
                    }
                    Job::Restore(id) => {
                        let result = match (&cp, turns.remove(&id)) {
                            (Some(cp), Some((before, after))) => {
                                // A turn that never got its end snapshot (still running): take it now.
                                let after = after.map(Ok).unwrap_or_else(|| cp.snapshot());
                                after.and_then(|after| cp.restore(&before, &after)).map(|paths| {
                                    paths.iter().map(|p| shown(&cp.root().join(p), &base)).collect()
                                })
                            }
                            _ => Err("no checkpoint for that turn".into()),
                        };
                        reply(Done::Restored(result));
                    }
                    Job::Clear => turns.clear(),
                }
            }
        });
        Worker { tx }
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }
}

/// `path` relative to `base` when inside it.
fn shown(path: &Path, base: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winfer-cp-{name}-{:x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restores_created_deleted_and_edited_files() {
        let work = tmp("work");
        let store = tmp("store");
        std::fs::write(work.join("keep.txt"), "one\n").unwrap();
        std::fs::write(work.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(work.join("untouched.txt"), "same\n").unwrap();
        let cp = Checkpoints::open(&work, &store, "s").unwrap();
        let before = cp.snapshot().unwrap();
        // The turn: an edit, a deletion, a new file in a new folder.
        std::fs::write(work.join("keep.txt"), "two\n").unwrap();
        std::fs::remove_file(work.join("gone.txt")).unwrap();
        std::fs::create_dir_all(work.join("new/deep")).unwrap();
        std::fs::write(work.join("new/deep/made.txt"), "hi\n").unwrap();
        let after = cp.snapshot().unwrap();
        // The user changes another file after the turn: that stays.
        std::fs::write(work.join("untouched.txt"), "mine\n").unwrap();
        let paths = cp.restore(&before, &after).unwrap();
        assert_eq!(paths, ["gone.txt", "keep.txt", "new/deep/made.txt"]);
        assert_eq!(std::fs::read_to_string(work.join("keep.txt")).unwrap(), "one\n");
        assert_eq!(std::fs::read_to_string(work.join("gone.txt")).unwrap(), "bye\n");
        assert!(!work.join("new").exists());
        assert_eq!(std::fs::read_to_string(work.join("untouched.txt")).unwrap(), "mine\n");
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&store);
    }

    #[test]
    fn worker_answers_in_order() {
        let work = tmp("worker");
        let store_dir = tmp("wstore");
        std::fs::write(work.join("a.txt"), "a\n").unwrap();
        let (tx, rx) = channel();
        let w = Worker::start(work.clone(), store_dir.clone(), "t".into(), move |d| {
            let _ = tx.send(d);
        });
        w.send(Job::Begin(1));
        assert!(matches!(rx.recv().unwrap(), Done::Began(1, true)));
        std::fs::write(work.join("b.txt"), "b\n").unwrap();
        w.send(Job::End(1));
        w.send(Job::Restore(1));
        match rx.recv().unwrap() {
            Done::Restored(Ok(paths)) => assert_eq!(paths, ["b.txt"]),
            _ => panic!("expected the restore"),
        }
        assert!(!work.join("b.txt").exists());
        w.send(Job::Restore(1));
        assert!(matches!(rx.recv().unwrap(), Done::Restored(Err(_))));
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&store_dir);
    }
}
