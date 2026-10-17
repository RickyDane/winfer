//! Agent skills: folders with a `SKILL.md` (YAML front matter with `name` and `description`) that
//! coding agents load from a skills folder in the user's home, e.g. `~/.claude/skills/<name>/`.
//!
//! winfer lists what's installed in every agent's folder and installs skills from GitHub repos:
//! every folder in a repo holding a `SKILL.md` is a skill, named after the folder. Installing
//! downloads that folder's files into the chosen agents' folders and leaves a marker file saying
//! where it came from, so it can be updated later.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use serde_json::Value;

/// Marks a skill folder winfer installed, with the repo, path and commit it came from.
const MARKER: &str = ".winfer-skill.json";
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_FILES: usize = 300;
/// Descriptions are read from at most this many skills of a repo.
const MAX_SKILLS: usize = 200;

/// A skills folder an agent reads.
struct Target {
    id: &'static str,
    label: &'static str,
    /// Relative to the home folder.
    dir: &'static str,
    /// Relative to the home folder: present when the agent is set up on this machine.
    home: &'static str,
}

const TARGETS: &[Target] = &[
    Target { id: "claude", label: "Claude Code", dir: ".claude/skills", home: ".claude" },
    Target { id: "agents", label: "Shared (.agents)", dir: ".agents/skills", home: ".agents" },
    Target { id: "codex", label: "Codex", dir: ".codex/skills", home: ".codex" },
    Target { id: "gemini", label: "Gemini / Antigravity", dir: ".gemini/skills", home: ".gemini" },
    Target { id: "pi", label: "pi", dir: ".pi/agent/skills", home: ".pi" },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub id: String,
    pub label: String,
    pub dir: String,
    /// The agent's own folder exists, so it's probably installed.
    pub detected: bool,
}

/// Where an installed skill came from.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Origin {
    pub repo: String,
    pub path: String,
    pub rev: String,
    /// The git tree hash of the skill's folder: it changes only when the skill does.
    #[serde(default)]
    pub hash: String,
}

/// One copy of a skill in one agent's folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCopy {
    pub target: String,
    pub dir: String,
    /// A symlink to a folder elsewhere (removing it leaves that folder alone).
    pub link: bool,
    pub origin: Option<Origin>,
}

/// An installed skill, with every agent folder it's in.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    pub name: String,
    pub description: String,
    pub copies: Vec<SkillCopy>,
}

/// A skill a repo offers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSkill {
    pub name: String,
    pub description: String,
    /// The skill's folder inside the repo.
    pub path: String,
    pub files: usize,
    /// The git tree hash of the skill's folder.
    pub hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoListing {
    pub repo: String,
    pub rev: String,
    pub skills: Vec<RepoSkill>,
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "couldn't find your home folder".to_string())
}

fn target(id: &str) -> Result<&'static Target, String> {
    TARGETS.iter().find(|t| t.id == id).ok_or_else(|| format!("unknown skills folder “{id}”"))
}

/// Skill names are folder names: lowercase letters, digits, `-`, `_` and `.`, not starting with a dot.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `name` and `description` from a SKILL.md's front matter (folded and quoted values included).
fn front_matter(text: &str) -> (Option<String>, String) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, String::new());
    }
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented && let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim();
            let value = if matches!(value, ">" | "|" | ">-" | "|-" | ">+" | "|+") { "" } else { value };
            fields.insert(key.clone(), value.to_string());
            current = Some(key);
            continue;
        }
        if let Some(key) = &current {
            let entry = fields.entry(key.clone()).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(line.trim());
        }
    }
    let unquote = |s: &str| {
        let s = s.trim();
        let quoted = s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')));
        if quoted { s[1..s.len() - 1].to_string() } else { s.to_string() }
    };
    let name = fields.get("name").map(|s| unquote(s)).filter(|s| !s.is_empty());
    (name, fields.get("description").map(|s| unquote(s)).unwrap_or_default())
}

// ---------------------------------------------------------------------------------------------
// Installed skills

fn scan_target(home: &Path, target: &Target, out: &mut BTreeMap<String, InstalledSkill>) {
    let root = home.join(target.dir);
    let Ok(entries) = std::fs::read_dir(&root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        if name.starts_with('.') || !dir.join("SKILL.md").is_file() {
            continue;
        }
        let link = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        let origin = std::fs::read(dir.join(MARKER)).ok().and_then(|b| serde_json::from_slice::<Origin>(&b).ok());
        let skill = out.entry(name.clone()).or_insert_with(|| InstalledSkill { name: name.clone(), description: String::new(), copies: Vec::new() });
        if skill.description.is_empty() && let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) {
            skill.description = front_matter(&text).1;
        }
        skill.copies.push(SkillCopy { target: target.id.to_string(), dir: dir.to_string_lossy().into_owned(), link, origin });
    }
}

fn list_installed(home: &Path) -> Vec<InstalledSkill> {
    let mut skills = BTreeMap::new();
    for target in TARGETS {
        scan_target(home, target, &mut skills);
    }
    skills.into_values().collect()
}

// ---------------------------------------------------------------------------------------------
// GitHub repos

/// `owner/repo` from `owner/repo`, `github.com/owner/repo` or a full GitHub URL (`.git`, `/tree/…`
/// and the like are dropped).
fn parse_repo(input: &str) -> Result<String, String> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    let mut parts = s.split('/');
    let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
        return Err(format!("“{}” isn't a GitHub repo; use owner/repo", input.trim()));
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok(owner) || !ok(repo) {
        return Err(format!("“{}” isn't a GitHub repo; use owner/repo", input.trim()));
    }
    Ok(format!("{owner}/{repo}"))
}

fn http_get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    agent
        .get(url)
        .header("User-Agent", "winfer")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "not found on GitHub (is the repo public?)".to_string(),
            ureq::Error::StatusCode(403 | 429) => "GitHub's rate limit was reached; try again in a while".to_string(),
            e => format!("couldn't reach GitHub: {e}"),
        })
}

fn github_json(url: &str) -> Result<Value, String> {
    http_get(url)?.body_mut().read_json::<Value>().map_err(|e| format!("GitHub's answer is invalid: {e}"))
}

/// Percent-encodes a path's segments for a URL.
fn url_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A repo's files at one commit: skill folder → its files (relative to it).
#[derive(Clone)]
struct Tree {
    rev: String,
    skills: BTreeMap<String, Vec<String>>,
    /// Folder → its git tree hash.
    hashes: HashMap<String, String>,
}

/// Finds every skill folder in a list of repo file paths. Hidden folders are skipped (except the
/// skill's own hidden files, which are dropped). When two folders share a name (a repo shipping a
/// copy under `plugins/…`), the shallower one wins.
fn skill_folders(paths: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut dirs: Vec<&str> = paths
        .iter()
        .filter_map(|p| p.strip_suffix("/SKILL.md"))
        .filter(|dir| !dir.split('/').any(|part| part.starts_with('.')))
        .filter(|dir| dir.rsplit('/').next().is_some_and(valid_name))
        .collect();
    dirs.sort_by_key(|d| (d.matches('/').count(), d.len()));
    let mut by_name: HashMap<&str, &str> = HashMap::new();
    for dir in dirs {
        by_name.entry(dir.rsplit('/').next().unwrap()).or_insert(dir);
    }
    let mut out = BTreeMap::new();
    for dir in by_name.into_values() {
        let prefix = format!("{dir}/");
        let mut files: Vec<String> = paths
            .iter()
            .filter_map(|p| p.strip_prefix(&prefix))
            .filter(|rel| !rel.split('/').any(|part| part.starts_with('.')))
            .map(str::to_string)
            .collect();
        files.sort();
        out.insert(dir.to_string(), files);
    }
    out
}

fn fetch_tree(repo: &str) -> Result<Tree, String> {
    let commit = github_json(&format!("https://api.github.com/repos/{repo}/commits/HEAD")).map_err(|e| format!("{repo}: {e}"))?;
    let rev = commit["sha"].as_str().ok_or("GitHub's answer has no commit")?.to_string();
    let tree = commit["commit"]["tree"]["sha"].as_str().ok_or("GitHub's answer has no tree")?;
    let tree = github_json(&format!("https://api.github.com/repos/{repo}/git/trees/{tree}?recursive=1"))?;
    let items = tree["tree"].as_array().ok_or("GitHub's answer has no files")?;
    let of_type = |kind: &'static str| items.iter().filter(move |item| item["type"] == kind);
    let paths: Vec<String> = of_type("blob").filter_map(|item| item["path"].as_str().map(str::to_string)).collect();
    let skills = skill_folders(&paths);
    let hashes = of_type("tree")
        .filter_map(|item| Some((item["path"].as_str()?.to_string(), item["sha"].as_str()?.to_string())))
        .filter(|(path, _)| skills.contains_key(path))
        .collect();
    Ok(Tree { rev, skills, hashes })
}

/// Listings are kept for a few minutes: GitHub allows 60 API calls an hour without a token, and
/// a listing takes two.
fn cached_tree(repo: &str, fresh: bool) -> Result<Tree, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, Tree)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if !fresh
        && let Some((at, tree)) = cache.lock().unwrap().get(repo)
        && at.elapsed() < std::time::Duration::from_secs(600)
    {
        return Ok(tree.clone());
    }
    let tree = fetch_tree(repo)?;
    cache.lock().unwrap().insert(repo.to_string(), (std::time::Instant::now(), tree.clone()));
    Ok(tree)
}

fn fetch_file(repo: &str, rev: &str, path: &str) -> Result<Vec<u8>, String> {
    http_get(&format!("https://raw.githubusercontent.com/{repo}/{rev}/{}", url_path(path)))?
        .body_mut()
        .with_config()
        .limit(MAX_FILE_BYTES)
        .read_to_vec()
        .map_err(|e| format!("couldn't download {path}: {e}"))
}

fn list_repo(repo: &str, fresh: bool) -> Result<RepoListing, String> {
    let Tree { rev, skills: folders, hashes } = cached_tree(repo, fresh)?;
    let folders: Vec<(String, usize)> = folders.into_iter().take(MAX_SKILLS).map(|(dir, files)| (dir, files.len())).collect();
    // Raw files aren't rate limited, so the SKILL.md files are read in parallel (a few at a time).
    let descriptions: Vec<String> = folders
        .chunks(16)
        .flat_map(|chunk| {
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|(dir, _)| {
                        let rev = &rev;
                        scope.spawn(move || {
                            fetch_file(repo, rev, &format!("{dir}/SKILL.md")).map(|b| front_matter(&String::from_utf8_lossy(&b)).1).unwrap_or_default()
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap_or_default()).collect::<Vec<_>>()
            })
        })
        .collect();
    let mut skills: Vec<RepoSkill> = folders
        .into_iter()
        .zip(descriptions)
        .map(|((path, files), description)| RepoSkill {
            name: path.rsplit('/').next().unwrap_or(&path).to_string(),
            description,
            hash: hashes.get(&path).cloned().unwrap_or_default(),
            path,
            files,
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(RepoListing { repo: repo.to_string(), rev, skills })
}

/// Resolves a `/`-separated relative path inside `dir`, refusing anything that escapes it.
fn resolve_inside(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut out = dir.to_path_buf();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(format!("{rel} leaves the skill folder")),
            p => out.push(p),
        }
    }
    Ok(out)
}

/// Removes a skill folder, or just the link when it's a symlink.
fn remove_skill_dir(dir: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(dir).map_err(|e| e.to_string())
    } else {
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())
    }
}

/// Puts a finished copy at `target`, replacing what was there only once it's complete.
fn place(staging: &Path, target: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(target).is_ok() {
        remove_skill_dir(target).map_err(|e| format!("couldn't replace {}: {e}", target.display()))?;
    }
    std::fs::rename(staging, target).map_err(|e| e.to_string())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let dest = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Writes a downloaded skill (`files`: path inside the skill → bytes) into every target folder.
fn write_skill(home: &Path, name: &str, files: &[(String, Vec<u8>)], origin: &Origin, targets: &[&Target]) -> Result<(), String> {
    if !files.iter().any(|(rel, _)| rel == "SKILL.md") {
        return Err(format!("{name} has no SKILL.md"));
    }
    for target in targets {
        let root = home.join(target.dir);
        std::fs::create_dir_all(&root).map_err(|e| format!("couldn't create {}: {e}", root.display()))?;
        let staging = root.join(format!(".install-{name}"));
        let _ = std::fs::remove_dir_all(&staging);
        let result = (|| {
            for (rel, bytes) in files {
                let path = resolve_inside(&staging, rel)?;
                std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            }
            std::fs::write(staging.join(MARKER), serde_json::to_vec_pretty(origin).unwrap()).map_err(|e| e.to_string())?;
            place(&staging, &root.join(name))
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result?;
    }
    Ok(())
}

fn install(home: &Path, repo: &str, path: &str, targets: &[String]) -> Result<String, String> {
    let repo = parse_repo(repo)?;
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    if !valid_name(&name) {
        return Err(format!("“{name}” isn't a valid skill name"));
    }
    let targets: Vec<&Target> = targets.iter().map(|t| target(t)).collect::<Result<_, _>>()?;
    if targets.is_empty() {
        return Err("pick at least one agent to install it for".into());
    }
    // Always the latest commit, so installing again updates.
    let Tree { rev, skills: folders, hashes } = cached_tree(&repo, true)?;
    let rels = folders.get(path).ok_or_else(|| format!("{repo} has no skill at {path}"))?;
    if rels.len() > MAX_FILES {
        return Err(format!("{name} has more than {MAX_FILES} files"));
    }
    let files: Vec<(String, Vec<u8>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = rels
            .iter()
            .map(|rel| {
                let (repo, rev) = (&repo, &rev);
                scope.spawn(move || fetch_file(repo, rev, &format!("{path}/{rel}")).map(|b| (rel.clone(), b)))
            })
            .collect();
        handles.into_iter().map(|h| h.join().map_err(|_| "download failed".to_string())?).collect::<Result<Vec<_>, String>>()
    })?;
    let origin = Origin { repo, path: path.to_string(), rev, hash: hashes.get(path).cloned().unwrap_or_default() };
    write_skill(home, &name, &files, &origin, &targets)?;
    Ok(name)
}

fn uninstall(home: &Path, name: &str, targets: &[String]) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("“{name}” isn't a valid skill name"));
    }
    for id in targets {
        let dir = home.join(target(id)?.dir).join(name);
        if std::fs::symlink_metadata(&dir).is_ok() {
            remove_skill_dir(&dir).map_err(|e| format!("couldn't remove {}: {e}", dir.display()))?;
        }
    }
    Ok(())
}

/// Copies an installed skill into more agents' folders (a symlinked copy is copied as files).
fn copy_to(home: &Path, name: &str, from: &str, targets: &[String]) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("“{name}” isn't a valid skill name"));
    }
    let source = home.join(target(from)?.dir).join(name);
    if !source.join("SKILL.md").is_file() {
        return Err(format!("{name} isn't installed for {from}"));
    }
    for id in targets.iter().filter(|t| *t != from) {
        let root = home.join(target(id)?.dir);
        std::fs::create_dir_all(&root).map_err(|e| format!("couldn't create {}: {e}", root.display()))?;
        let staging = root.join(format!(".install-{name}"));
        let _ = std::fs::remove_dir_all(&staging);
        let result = copy_dir(&source, &staging).and_then(|_| place(&staging, &root.join(name)));
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result?;
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------------------------
// Commands

/// The agents' skills folders.
#[tauri::command]
pub fn skills_targets() -> Result<Vec<TargetInfo>, String> {
    let home = home()?;
    Ok(TARGETS
        .iter()
        .map(|t| TargetInfo { id: t.id.into(), label: t.label.into(), dir: home.join(t.dir).to_string_lossy().into_owned(), detected: home.join(t.home).is_dir() })
        .collect())
}

/// Every skill installed in any agent's folder.
#[tauri::command]
pub async fn skills_list() -> Result<Vec<InstalledSkill>, String> {
    blocking(|| Ok(list_installed(&home()?))).await
}

/// The skills a GitHub repo offers (`fresh` skips the few-minute cache).
#[tauri::command]
pub async fn skills_repo(repo: String, fresh: Option<bool>) -> Result<RepoListing, String> {
    blocking(move || list_repo(&parse_repo(&repo)?, fresh.unwrap_or(false))).await
}

/// Installs (or updates) a repo's skill for the given agents; returns its name.
#[tauri::command]
pub async fn skills_install(repo: String, path: String, targets: Vec<String>) -> Result<String, String> {
    blocking(move || install(&home()?, &repo, &path, &targets)).await
}

/// Removes a skill from the given agents' folders.
#[tauri::command]
pub async fn skills_uninstall(name: String, targets: Vec<String>) -> Result<(), String> {
    blocking(move || uninstall(&home()?, &name, &targets)).await
}

/// Copies an installed skill from one agent's folder into others.
#[tauri::command]
pub async fn skills_copy(name: String, from: String, targets: Vec<String>) -> Result<(), String> {
    blocking(move || copy_to(&home()?, &name, &from, &targets)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winfer-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_front_matter() {
        let text = "---\nname: caveman\ndescription: >\n  Terse voice.\n  Stays on.\nallowed-tools: Bash\n---\n# body";
        assert_eq!(front_matter(text), (Some("caveman".into()), "Terse voice. Stays on.".into()));
        assert_eq!(front_matter("---\nname: \"x\"\ndescription: 'Quoted: yes'\n---"), (Some("x".into()), "Quoted: yes".into()));
        assert_eq!(front_matter("# no front matter"), (None, String::new()));
    }

    #[test]
    fn parses_repos() {
        assert_eq!(parse_repo("JuliusBrussee/caveman").unwrap(), "JuliusBrussee/caveman");
        assert_eq!(parse_repo("https://github.com/anthropics/skills.git").unwrap(), "anthropics/skills");
        assert_eq!(parse_repo("github.com/a/b/tree/main/skills").unwrap(), "a/b");
        assert!(parse_repo("caveman").is_err());
        assert!(parse_repo("a b/c").is_err());
    }

    #[test]
    fn finds_skill_folders() {
        let paths = [
            "skills/caveman/SKILL.md",
            "skills/caveman/README.md",
            "skills/caveman/.DS_Store",
            "plugins/caveman/skills/caveman/SKILL.md",
            "plugins/caveman/skills/megacave/SKILL.md",
            "skills/pdf/SKILL.md",
            "skills/pdf/scripts/fill.py",
            ".github/skills/hidden/SKILL.md",
            "Bad Name/SKILL.md",
            "SKILL.md",
        ]
        .map(String::from);
        let found = skill_folders(&paths);
        assert_eq!(found.keys().collect::<Vec<_>>(), ["plugins/caveman/skills/megacave", "skills/caveman", "skills/pdf"]);
        assert_eq!(found["skills/caveman"], ["README.md", "SKILL.md"]);
        assert_eq!(found["skills/pdf"], ["SKILL.md", "scripts/fill.py"]);
    }

    #[test]
    fn writes_lists_copies_and_removes_skills() {
        let home = temp_home("install");
        let origin = Origin { repo: "a/b".into(), path: "skills/demo".into(), rev: "abc".into(), hash: "def".into() };
        let files = vec![("SKILL.md".to_string(), b"---\nname: demo\ndescription: A demo.\n---\n".to_vec()), ("scripts/run.sh".to_string(), b"echo".to_vec())];
        write_skill(&home, "demo", &files, &origin, &[target("claude").unwrap()]).unwrap();
        assert!(home.join(".claude/skills/demo/scripts/run.sh").is_file());
        assert!(write_skill(&home, "x", &[("../evil".into(), vec![]), ("SKILL.md".into(), vec![])], &origin, &[target("claude").unwrap()]).is_err());
        assert!(!home.join(".claude/skills/.install-x").exists());

        copy_to(&home, "demo", "claude", &["codex".into()]).unwrap();
        let skills = list_installed(&home);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "A demo.");
        assert_eq!(skills[0].copies.iter().map(|c| c.target.as_str()).collect::<Vec<_>>(), ["claude", "codex"]);
        assert_eq!(skills[0].copies[0].origin.as_ref(), Some(&origin));

        // Reinstalling replaces the folder.
        write_skill(&home, "demo", &files[..1], &origin, &[target("claude").unwrap()]).unwrap();
        assert!(!home.join(".claude/skills/demo/scripts").exists());

        uninstall(&home, "demo", &["claude".into(), "codex".into()]).unwrap();
        assert!(list_installed(&home).is_empty());
        assert!(uninstall(&home, "../x", &["claude".into()]).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn removing_a_linked_skill_keeps_its_source() {
        let home = temp_home("link");
        let source = home.join("elsewhere/linked");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "---\nname: linked\n---").unwrap();
        std::fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(&source, home.join(".claude/skills/linked")).unwrap();
        assert!(list_installed(&home)[0].copies[0].link);
        uninstall(&home, "linked", &["claude".into()]).unwrap();
        assert!(source.join("SKILL.md").is_file());
        let _ = std::fs::remove_dir_all(&home);
    }
}

/// Hits GitHub: `cargo test --lib skills::network -- --ignored`.
#[cfg(test)]
mod network {
    use super::*;

    #[test]
    #[ignore]
    fn lists_and_installs_caveman() {
        let listing = list_repo("JuliusBrussee/caveman", true).unwrap();
        let caveman = listing.skills.iter().find(|s| s.name == "caveman").unwrap();
        assert_eq!(caveman.path, "skills/caveman");
        assert!(caveman.description.contains("caveman") && caveman.hash.len() == 40);
        let home = std::env::temp_dir().join(format!("winfer-skills-net-{}", std::process::id()));
        install(&home, "github.com/JuliusBrussee/caveman", "skills/caveman", &["claude".into()]).unwrap();
        assert!(home.join(".claude/skills/caveman/SKILL.md").is_file());
        assert_eq!(list_installed(&home)[0].copies[0].origin.as_ref().unwrap().repo, "JuliusBrussee/caveman");
        let _ = std::fs::remove_dir_all(&home);
    }
}
