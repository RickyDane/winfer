//! Skills for Claude Code in lean mode. Lean mode leaves out the user's settings (and with them
//! `~/.claude/skills` and plugin skills), so it either loads them all (`--setting-sources user,…`)
//! or only the ones picked in Settings: those are linked into a folder of their own, passed with
//! `--add-dir`, whose `.claude/skills` Claude Code loads like a project's. Lean Codex panes follow
//! the same choice: the user's Codex skills not picked are turned off with `skills.config`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Which skills Claude Code gets in lean mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeanSkills {
    /// Every skill the user has (loads the user's settings, hooks and plugins too).
    All,
    /// Only these (ids from [`available`]); none turns skills off.
    Only(Vec<String>),
}

/// A skill Claude Code would load from the user's setup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSkill {
    /// `name` for `~/.claude/skills/<name>`, `plugin:name` for a plugin's.
    pub id: String,
    pub name: String,
    pub description: String,
    /// `user` or the plugin's name.
    pub source: String,
    pub dir: PathBuf,
}

fn skill_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<_> = entries
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(name, dir)| !name.starts_with('.') && dir.join("SKILL.md").is_file())
        .collect();
    out.sort();
    out
}

fn describe(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("SKILL.md")).map(|t| crate::skills::front_matter(&t).1).unwrap_or_default()
}

/// Install folders of the user's enabled Claude Code plugins, by plugin name.
fn enabled_plugins(home: &Path) -> Vec<(String, PathBuf)> {
    let read = |p: PathBuf| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let enabled = read(home.join(".claude/settings.json")).and_then(|s| s.get("enabledPlugins").cloned());
    let Some(Value::Object(enabled)) = enabled else { return Vec::new() };
    let installed = read(home.join(".claude/plugins/installed_plugins.json"));
    let mut out = Vec::new();
    for (key, on) in enabled {
        if on != Value::Bool(true) {
            continue;
        }
        let path = installed
            .as_ref()
            .and_then(|i| i.pointer(&format!("/plugins/{}", key.replace('~', "~0").replace('/', "~1"))))
            .and_then(|v| v.as_array()?.first()?.get("installPath")?.as_str().map(PathBuf::from));
        if let Some(path) = path {
            out.push((key.split('@').next().unwrap_or(&key).to_string(), path));
        }
    }
    out.sort();
    out
}

/// The skills in `~/.claude/skills` and in the user's enabled plugins.
pub fn available_in(home: &Path) -> Vec<ClaudeSkill> {
    let mut out: Vec<ClaudeSkill> = skill_dirs(&home.join(".claude/skills"))
        .into_iter()
        .map(|(name, dir)| ClaudeSkill { id: name.clone(), description: describe(&dir), name, source: "user".into(), dir })
        .collect();
    for (plugin, root) in enabled_plugins(home) {
        for (name, dir) in skill_dirs(&root.join("skills")) {
            out.push(ClaudeSkill { id: format!("{plugin}:{name}"), description: describe(&dir), name, source: plugin.clone(), dir });
        }
    }
    out
}

pub fn available() -> Vec<ClaudeSkill> {
    dirs::home_dir().map(|h| available_in(&h)).unwrap_or_default()
}

/// The user skill folders Codex loads (its bundled `.system` skills are left out).
const CODEX_DIRS: &[&str] = &[".codex/skills", ".agents/skills"];

/// The skills Codex loads from the user's folders, id `codex:name`. A name can come twice (once per folder).
pub fn codex_in(home: &Path) -> Vec<ClaudeSkill> {
    CODEX_DIRS
        .iter()
        .flat_map(|d| skill_dirs(&home.join(d)))
        .map(|(name, dir)| ClaudeSkill { id: format!("codex:{name}"), description: describe(&dir), name, source: "codex".into(), dir })
        .collect()
}

/// The skills to pick from in Settings: Claude Code's, then the Codex ones it has no skill of that name for.
pub fn pickable_in(home: &Path) -> Vec<ClaudeSkill> {
    let mut out = available_in(home);
    for skill in codex_in(home) {
        if !out.iter().any(|o| o.name == skill.name) {
            out.push(skill);
        }
    }
    out
}

pub fn pickable() -> Vec<ClaudeSkill> {
    dirs::home_dir().map(|h| pickable_in(&h)).unwrap_or_default()
}

/// Codex `-c` arguments that turn off each of the user's Codex skills not `selected`. Skills match
/// by name, so picking a Claude Code skill keeps Codex's skill of the same name too.
pub fn codex_args_in(home: &Path, selected: &[String]) -> Vec<String> {
    let names: Vec<&str> = selected.iter().map(|id| id.rsplit(':').next().unwrap_or(id)).collect();
    let off: Vec<String> = codex_in(home)
        .into_iter()
        .filter(|s| !names.contains(&s.name.as_str()))
        .map(|s| format!("{{path={},enabled=false}}", serde_json::json!(s.dir.join("SKILL.md"))))
        .collect();
    if off.is_empty() {
        return Vec::new();
    }
    vec!["-c".into(), format!("skills.config=[{}]", off.join(","))]
}

/// [`codex_args_in`] for the lean skill choice in Settings; none when all skills are on.
pub fn codex_args(skills: &LeanSkills) -> Vec<String> {
    match (skills, dirs::home_dir()) {
        (LeanSkills::Only(ids), Some(home)) => codex_args_in(&home, ids),
        _ => Vec::new(),
    }
}

/// A folder whose `.claude/skills` links to just the `selected` skills, for `--add-dir`. The same
/// selection always gets the same folder, so Claude Code's prompt (and its cache) stays the same.
/// `None` if none of them exist any more.
pub fn prepare_in(home: &Path, base: &Path, selected: &[String]) -> Option<PathBuf> {
    let skills: Vec<_> = available_in(home).into_iter().filter(|s| selected.contains(&s.id)).collect();
    if skills.is_empty() {
        return None;
    }
    let mut ids: Vec<&str> = skills.iter().map(|s| s.id.as_str()).collect();
    ids.sort();
    let hash = Sha256::digest(ids.join("\n").as_bytes());
    let key: String = hash.iter().take(6).map(|b| format!("{b:02x}")).collect();
    let root = base.join(key);
    let links = root.join(".claude").join("skills");
    std::fs::create_dir_all(&links).ok()?;
    for skill in &skills {
        // A plugin skill named like a user skill gets the plugin's name in front.
        let clash = skills.iter().any(|o| o.id != skill.id && o.name == skill.name);
        let name = if clash && skill.source != "user" { format!("{}-{}", skill.source, skill.name) } else { skill.name.clone() };
        let link = links.join(&name);
        if std::fs::read_link(&link).is_ok_and(|t| t == skill.dir) {
            continue;
        }
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&skill.dir, &link);
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&skill.dir, &link);
    }
    Some(root)
}

pub fn prepare(selected: &[String]) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    prepare_in(&home, &super::config::dir().join("lean-skills"), selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(dir: &Path, desc: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: x\ndescription: {desc}\n---\nbody")).unwrap();
    }

    #[test]
    fn finds_user_and_plugin_skills_and_links_a_selection() {
        let home = std::env::temp_dir().join(format!("winfer-lean-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        skill(&home.join(".claude/skills/graphify"), "Graphs");
        skill(&home.join(".claude/skills/caveman"), "User caveman");
        let plugin = home.join("cache/caveman");
        skill(&plugin.join("skills/caveman"), "Talk less");
        skill(&home.join("cache/off/skills/nope"), "Disabled");
        std::fs::create_dir_all(home.join(".claude/plugins")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), r#"{"enabledPlugins":{"caveman@caveman":true,"off@x":false}}"#).unwrap();
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            serde_json::json!({ "plugins": {
                "caveman@caveman": [{ "installPath": plugin }],
                "off@x": [{ "installPath": home.join("cache/off") }],
            }})
            .to_string(),
        )
        .unwrap();

        let all = available_in(&home);
        let ids: Vec<_> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["caveman", "graphify", "caveman:caveman"]);
        assert_eq!(all[2].description, "Talk less");

        let base = home.join("base");
        let sel = vec!["graphify".to_string(), "caveman:caveman".to_string(), "gone".to_string()];
        let dir = prepare_in(&home, &base, &sel).unwrap();
        assert!(dir.join(".claude/skills/graphify/SKILL.md").is_file());
        assert!(dir.join(".claude/skills/caveman/SKILL.md").is_file());
        // Same selection, same folder; order doesn't matter.
        assert_eq!(prepare_in(&home, &base, &[sel[1].clone(), sel[0].clone()]), Some(dir));
        // A plugin skill clashing with a user skill gets the plugin's name in front.
        let both = prepare_in(&home, &base, &["caveman".into(), "caveman:caveman".into()]).unwrap();
        assert!(both.join(".claude/skills/caveman-caveman/SKILL.md").is_file());
        assert_eq!(prepare_in(&home, &base, &["gone".into()]), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn codex_keeps_only_picked_skills_by_name() {
        let home = std::env::temp_dir().join(format!("winfer-codex-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        skill(&home.join(".claude/skills/graphify"), "Graphs");
        skill(&home.join(".codex/skills/graphify"), "Graphs");
        skill(&home.join(".codex/skills/wrangler"), "Workers");
        skill(&home.join(".agents/skills/caveman"), "Talk less");
        skill(&home.join(".codex/skills/.system/imagegen"), "Bundled");

        let ids: Vec<_> = pickable_in(&home).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["graphify", "codex:wrangler", "codex:caveman"]);

        let args = codex_args_in(&home, &["graphify".into(), "plugin:caveman".into()]);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("skills.config=[{path=\""));
        assert!(args[1].contains("/.codex/skills/wrangler/SKILL.md\",enabled=false}"));
        assert!(!args[1].contains("graphify") && !args[1].contains("caveman") && !args[1].contains(".system"));
        assert!(codex_args_in(&home, &["graphify".into(), "codex:wrangler".into(), "caveman".into()]).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }
}
