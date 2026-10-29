//! Several Claude Code / Codex sign-ins, used in order: when one account's plan runs out, agents
//! continue on the next.
//!
//! The first account of each agent is its default sign-in (`~/.claude`, `~/.codex`). Every other
//! account gets its own config dir (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`) under winfer's data dir,
//! which keeps its credentials apart: Claude Code names its Keychain entry after the config dir,
//! Codex keeps `auth.json` in it. Sessions, settings and skills are symlinked from the default
//! dir, so a session started on one account resumes on another.
//!
//! The list lives in a JSON file (not the app's database) because `winfer code` reads it too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    /// "claude" or "codex".
    pub provider: String,
    pub label: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    /// In failover order.
    #[serde(default)]
    accounts: Vec<Account>,
    /// Account id → when its usage resets (unix ms); 0 when unknown.
    #[serde(default)]
    exhausted: BTreeMap<String, i64>,
}

/// An account as the app shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    #[serde(flatten)]
    pub account: Account,
    pub dir: String,
    pub signed_in: bool,
    /// Out of usage until (unix ms; 0 = unknown), if so.
    pub exhausted_until: Option<i64>,
}

pub const PROVIDERS: [&str; 2] = ["claude", "codex"];

fn base() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("winfer-accounts-test-{}", std::process::id()));
    }
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("dev.winfer.app")
}

fn file_path() -> PathBuf {
    base().join("accounts.json")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// An exhausted account with no known reset is retried after this long.
const UNKNOWN_RESET_MS: i64 = 30 * 60 * 1000;

fn load() -> File {
    let mut file: File = std::fs::read_to_string(file_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for provider in PROVIDERS {
        if !file.accounts.iter().any(|a| a.provider == provider && a.id == default_id(provider)) {
            let at = file.accounts.iter().position(|a| a.provider == provider).unwrap_or(file.accounts.len());
            file.accounts.insert(at, Account { id: default_id(provider), provider: provider.into(), label: "Default".into(), enabled: true });
        }
    }
    file
}

fn save(file: &File) -> Result<(), String> {
    let path = file_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// The default account's id for an agent ("claude:default").
pub fn default_id(provider: &str) -> String {
    format!("{provider}:{DEFAULT}")
}

pub fn provider_of(id: &str) -> &str {
    id.split(':').next().unwrap_or(id)
}

fn is_default(id: &str) -> bool {
    id.ends_with(&format!(":{DEFAULT}"))
}

/// The agent's own config dir: what it uses with no override.
pub fn default_dir(provider: &str) -> Option<PathBuf> {
    let (var, name) = match provider {
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude"),
        "codex" => ("CODEX_HOME", ".codex"),
        _ => return None,
    };
    std::env::var_os(var).map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(name)))
}

/// Where an account keeps its sign-in and state.
pub fn dir(id: &str) -> Option<PathBuf> {
    let provider = provider_of(id);
    if is_default(id) {
        return default_dir(provider);
    }
    let name = id.split_once(':')?.1;
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| base().join("accounts").join(provider).join(name))
}

/// Whether the agent itself sees this dir as overridden (Claude Code's Keychain naming).
pub fn overridden(id: &str) -> bool {
    !is_default(id) || std::env::var_os(if provider_of(id) == "claude" { "CLAUDE_CONFIG_DIR" } else { "CODEX_HOME" }).is_some()
}

/// What an account shares with the default dir. Never credentials or the account record.
fn shared(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["projects", "settings.json", "CLAUDE.md", "agents", "commands", "skills", "plugins", "todos", "file-history", "shell-snapshots", "plans"],
        "codex" => &["sessions", "archived_sessions", "config.toml", "AGENTS.md", "prompts", "skills", "history.jsonl", "rules"],
        _ => &[],
    }
}

/// Creates the account's dir and links its shared state from the default dir. Idempotent.
pub fn ensure_links(id: &str) -> Result<(), String> {
    if is_default(id) {
        return Ok(());
    }
    let provider = provider_of(id);
    let (Some(dir), Some(primary)) = (dir(id), default_dir(provider)) else { return Err(format!("Unknown account {id}")) };
    link_shared(&primary, &dir, shared(provider))
}

fn link_shared(primary: &Path, dir: &Path, names: &[&str]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for name in names {
        let source = primary.join(name);
        let target = dir.join(name);
        if target.symlink_metadata().is_ok() {
            continue;
        }
        // Folders the agent would create later are made now, so they're shared from the start.
        if !source.exists() {
            if name.contains('.') {
                continue;
            }
            std::fs::create_dir_all(&source).map_err(|e| e.to_string())?;
        }
        symlink(&source, &target).map_err(|e| format!("Couldn't link {name}: {e}"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() { std::os::windows::fs::symlink_dir(source, target) } else { std::os::windows::fs::symlink_file(source, target) }
}

/// Claude Code's account record (`.claude.json`): copied once without the sign-in, so a new
/// account keeps project trust and MCP servers and skips onboarding.
fn seed_claude_state(dir: &Path) {
    let target = dir.join(".claude.json");
    if target.exists() {
        return;
    }
    let source = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        default_dir("claude").map(|d| d.join(".claude.json"))
    } else {
        dirs::home_dir().map(|h| h.join(".claude.json"))
    };
    let Some(mut state) = source.and_then(|s| std::fs::read_to_string(s).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return };
    if let Some(obj) = state.as_object_mut() {
        for key in ["oauthAccount", "userID", "primaryApiKey", "customApiKeyResponses", "s1mAccessCache", "cachedGrowthBookFeatures", "cachedStatsigGates"] {
            obj.remove(key);
        }
    }
    let _ = std::fs::write(target, serde_json::to_vec_pretty(&state).unwrap_or_default());
}

/// The environment that runs an agent on this account: nothing for the default sign-in.
pub fn env_for(id: Option<&str>) -> Vec<(String, String)> {
    let Some(id) = id.filter(|id| !is_default(id)) else { return Vec::new() };
    let Some(dir) = dir(id) else { return Vec::new() };
    if let Err(e) = ensure_links(id) {
        eprintln!("winfer: account {id}: {e}");
    }
    let var = if provider_of(id) == "claude" { "CLAUDE_CONFIG_DIR" } else { "CODEX_HOME" };
    vec![(var.to_string(), dir.to_string_lossy().into_owned())]
}

fn exhausted_until(file: &File, id: &str, now: i64) -> Option<i64> {
    let until = *file.exhausted.get(id)?;
    let effective = if until > 0 { until } else { i64::MAX };
    (effective > now).then_some(until)
}

fn signed_in(id: &str) -> bool {
    let Some(dir) = dir(id) else { return false };
    match provider_of(id) {
        "codex" => dir.join("auth.json").is_file(),
        // Keychain sign-ins can't be checked without a prompt; the account record says enough.
        _ => is_default(id) || std::fs::read_to_string(dir.join(".claude.json")).ok().is_some_and(|t| t.contains("\"oauthAccount\"")) || dir.join(".credentials.json").is_file(),
    }
}

/// The accounts to try for an agent, in order: enabled and not out of usage.
pub fn pick(provider: &str, exclude: &[&str]) -> Option<String> {
    let file = load();
    let now = now_ms();
    file.accounts
        .iter()
        .filter(|a| a.provider == provider && a.enabled && !exclude.contains(&a.id.as_str()))
        .find(|a| exhausted_until(&file, &a.id, now).is_none() && signed_in(&a.id))
        .map(|a| a.id.clone())
}

/// Whether the account is enabled, signed in and has usage left.
pub fn usable(id: &str) -> bool {
    let file = load();
    file.accounts.iter().any(|a| a.id == id && a.enabled) && exhausted_until(&file, id, now_ms()).is_none() && signed_in(id)
}

/// Whether there's more than the default account for this agent.
pub fn has_alternatives(provider: &str) -> bool {
    load().accounts.iter().filter(|a| a.provider == provider && a.enabled).count() > 1
}

pub fn label(id: &str) -> String {
    load().accounts.into_iter().find(|a| a.id == id).map(|a| a.label).unwrap_or_else(|| id.to_string())
}

/// Records that an account ran out of usage (until `reset_at`, unix ms, if known).
pub fn mark_exhausted(id: &str, reset_at: Option<i64>) {
    let mut file = load();
    let until = reset_at.filter(|&t| t > now_ms()).unwrap_or_else(|| now_ms() + UNKNOWN_RESET_MS);
    file.exhausted.insert(id.to_string(), until);
    let _ = save(&file);
}

pub fn clear_exhausted(id: &str) {
    let mut file = load();
    if file.exhausted.remove(id).is_some() {
        let _ = save(&file);
    }
}

pub fn list() -> Vec<Entry> {
    let file = load();
    let now = now_ms();
    file.accounts
        .iter()
        .map(|a| Entry {
            account: a.clone(),
            dir: dir(&a.id).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
            signed_in: signed_in(&a.id),
            exhausted_until: exhausted_until(&file, &a.id, now),
        })
        .collect()
}

fn add(provider: &str, label: &str) -> Result<Account, String> {
    if !PROVIDERS.contains(&provider) {
        return Err(format!("Accounts aren't supported for {provider}"));
    }
    let mut file = load();
    let id = (2..).map(|n| format!("{provider}:account-{n}")).find(|id| !file.accounts.iter().any(|a| &a.id == id) && !dir(id).is_some_and(|d| d.exists())).unwrap();
    let account = Account { id: id.clone(), provider: provider.into(), label: if label.trim().is_empty() { format!("Account {}", id.rsplit('-').next().unwrap_or("2")) } else { label.trim().into() }, enabled: true };
    ensure_links(&id)?;
    if provider == "claude" {
        seed_claude_state(&dir(&id).unwrap());
    }
    let at = file.accounts.iter().rposition(|a| a.provider == provider).map_or(file.accounts.len(), |i| i + 1);
    file.accounts.insert(at, account.clone());
    save(&file)?;
    Ok(account)
}

// Commands ---------------------------------------------------------------------------------------

#[tauri::command]
pub fn accounts_list() -> Vec<Entry> {
    list()
}

#[tauri::command]
pub fn accounts_add(provider: String, label: String) -> Result<Account, String> {
    add(&provider, &label)
}

/// Forgets an account and deletes its own files (the shared ones are links, left alone).
#[tauri::command]
pub fn accounts_remove(id: String) -> Result<(), String> {
    if is_default(&id) {
        return Err("The default account can't be removed.".into());
    }
    let mut file = load();
    file.accounts.retain(|a| a.id != id);
    file.exhausted.remove(&id);
    save(&file)?;
    if let Some(dir) = dir(&id) {
        remove_account_dir(&dir);
    }
    Ok(())
}

/// Deletes an account dir without following its links into the shared state.
fn remove_account_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_symlink() => {
                    let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path));
                }
                Ok(t) if t.is_dir() => remove_account_dir(&path),
                _ => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

#[tauri::command]
pub fn accounts_update(id: String, label: Option<String>, enabled: Option<bool>) -> Result<(), String> {
    let mut file = load();
    let account = file.accounts.iter_mut().find(|a| a.id == id).ok_or("Unknown account")?;
    if let Some(label) = label.filter(|l| !l.trim().is_empty()) {
        account.label = label.trim().into();
    }
    if let Some(enabled) = enabled {
        account.enabled = enabled;
    }
    save(&file)
}

/// Sets the failover order of one agent's accounts.
#[tauri::command]
pub fn accounts_reorder(provider: String, ids: Vec<String>) -> Result<(), String> {
    let mut file = load();
    let (mut mine, others): (Vec<Account>, Vec<Account>) = file.accounts.into_iter().partition(|a| a.provider == provider);
    mine.sort_by_key(|a| ids.iter().position(|id| id == &a.id).unwrap_or(usize::MAX));
    file.accounts = others.into_iter().chain(mine).collect();
    save(&file)
}

#[tauri::command]
pub fn accounts_clear_exhausted(id: String) {
    clear_exhausted(&id);
}

#[tauri::command]
pub fn accounts_mark_exhausted(id: String, reset_at: Option<i64>) {
    mark_exhausted(&id, reset_at);
}

/// The next account to use for an agent, skipping `exclude`.
#[tauri::command]
pub fn accounts_pick(provider: String, exclude: Vec<String>) -> Option<String> {
    pick(&provider, &exclude.iter().map(String::as_str).collect::<Vec<_>>())
}

/// How to sign an account in: the command to run in a terminal, and its environment.
#[derive(Debug, Serialize)]
pub struct Login {
    pub command: String,
    pub env: BTreeMap<String, String>,
}

#[tauri::command]
pub fn accounts_login(id: String) -> Result<Login, String> {
    ensure_links(&id)?;
    let command = match provider_of(&id) {
        "claude" => "claude /login",
        "codex" => "codex login",
        p => return Err(format!("Accounts aren't supported for {p}")),
    };
    let mut env: BTreeMap<String, String> = env_for(Some(&id)).into_iter().collect();
    if is_default(&id) {
        env.clear();
    }
    Ok(Login { command: command.into(), env })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_shared_state_but_never_credentials() {
        let root = std::env::temp_dir().join(format!("winfer-accounts-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let primary = root.join("primary");
        std::fs::create_dir_all(primary.join("projects")).unwrap();
        std::fs::write(primary.join("settings.json"), "{}").unwrap();
        std::fs::write(primary.join(".credentials.json"), "secret").unwrap();
        let dir = root.join("acct");
        link_shared(&primary, &dir, shared("claude")).unwrap();
        // Idempotent.
        link_shared(&primary, &dir, shared("claude")).unwrap();
        assert!(dir.join("projects").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(dir.join("settings.json").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(dir.join("skills").exists(), "missing folders are created and shared");
        assert!(!dir.join("CLAUDE.md").exists(), "missing files aren't linked");
        assert!(!dir.join(".credentials.json").exists());
        remove_account_dir(&dir);
        assert!(!dir.exists());
        assert!(primary.join("projects").is_dir(), "removing an account leaves shared state");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn exhaustion_expires() {
        let mut file = File::default();
        file.exhausted.insert("claude:account-2".into(), 1_000);
        assert_eq!(exhausted_until(&file, "claude:account-2", 500), Some(1_000));
        assert_eq!(exhausted_until(&file, "claude:account-2", 2_000), None);
        assert_eq!(exhausted_until(&file, "claude:other", 0), None);
    }

    #[test]
    fn ids_map_to_dirs() {
        assert_eq!(provider_of("codex:account-2"), "codex");
        assert!(dir("claude:account-2").unwrap().ends_with("accounts/claude/account-2"));
        assert!(dir("claude:../x").is_none());
        assert!(env_for(Some("claude:default")).is_empty());
        assert!(env_for(None).is_empty());
    }
}
