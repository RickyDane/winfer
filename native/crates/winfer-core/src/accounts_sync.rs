//! Agent sign-ins shared between paired machines (headless H4): a desktop pushes an account's
//! sign-in to a server (`account.push`), and from then on the two are linked. Claude and Codex
//! rotate their refresh tokens, so whichever side refreshes sends the new sign-in to the other
//! (`account.sync`); otherwise each would sign the other out.
//!
//! A sign-in travels as named parts: Claude `credentials` (the OAuth JSON Claude Code keeps in the
//! macOS Keychain or `.credentials.json`) and `account` (its `oauthAccount` record); Codex `auth`
//! (`auth.json`).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use winfer_backend::accounts;

pub type Parts = BTreeMap<String, String>;

/// A pushed account and the other machine's copy of it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub peer: String,
    pub local: String,
    pub remote: String,
    pub provider: String,
    /// Fingerprint of the sign-in both sides last had.
    pub fp: String,
    /// The other machine pushed it here (rather than this one pushing it there).
    #[serde(default)]
    pub received: bool,
}

static LINKS: Mutex<Option<Vec<Link>>> = Mutex::new(None);

fn links_path() -> PathBuf {
    crate::backend::data_dir().join("account-links.json")
}

fn with_links<R>(f: impl FnOnce(&mut Vec<Link>) -> R) -> R {
    let mut guard = LINKS.lock().unwrap();
    let links = guard.get_or_insert_with(|| std::fs::read_to_string(links_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    let before = links.clone();
    let out = f(links);
    if *links != before {
        let _ = std::fs::write(links_path(), serde_json::to_string_pretty(links).unwrap_or_default());
    }
    out
}

pub fn links() -> Vec<Link> {
    with_links(|l| l.clone())
}

/// Forgets the links with machines no longer paired. A server also signs out of what they
/// pushed to it: their sign-ins leave with them.
pub fn retain_peers(paired: &[String]) {
    let gone: Vec<Link> = links().into_iter().filter(|l| !paired.contains(&l.peer)).collect();
    if gone.is_empty() {
        return;
    }
    with_links(|l| l.retain(|x| paired.contains(&x.peer)));
    if crate::backend::profile().server {
        for link in gone.iter().filter(|l| l.received) {
            sign_out(&link.local);
        }
    }
}

/// Removes an account's sign-in here: the account itself, or the default account's credentials.
fn sign_out(id: &str) {
    if id != accounts::default_id(accounts::provider_of(id)) {
        let _ = accounts::accounts_remove(id.to_string());
        return;
    }
    let Some(dir) = accounts::dir(id) else { return };
    for file in [".credentials.json", "auth.json"] {
        let _ = std::fs::remove_file(dir.join(file));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("security").args(["delete-generic-password", "-s", &claude_service(id, &dir)]).output();
    }
}

pub fn fingerprint(parts: &Parts) -> String {
    let mut h = Sha256::new();
    for (k, v) in parts {
        h.update((k.len() as u32).to_be_bytes());
        h.update(k.as_bytes());
        h.update((v.len() as u32).to_be_bytes());
        h.update(v.as_bytes());
    }
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

fn is_default(id: &str) -> bool {
    id == accounts::default_id(accounts::provider_of(id))
}

/// Claude Code's own record of the account (`.claude.json`): next to the config dir for the
/// default sign-in, inside it otherwise.
fn claude_json(id: &str) -> Option<PathBuf> {
    if is_default(id) && std::env::var_os("CLAUDE_CONFIG_DIR").is_none() {
        return dirs::home_dir().map(|h| h.join(".claude.json"));
    }
    accounts::dir(id).map(|d| d.join(".claude.json"))
}

/// The Keychain item Claude Code keeps an account's sign-in in (macOS).
#[cfg(target_os = "macos")]
fn claude_service(id: &str, dir: &Path) -> String {
    if accounts::overridden(id) {
        let hash = Sha256::digest(dir.to_string_lossy().as_bytes());
        format!("Claude Code-credentials-{}", &hash.iter().map(|b| format!("{b:02x}")).collect::<String>()[..8])
    } else {
        "Claude Code-credentials".into()
    }
}

#[cfg(target_os = "macos")]
fn user() -> String {
    std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "winfer".into())
}

#[cfg(target_os = "macos")]
fn keychain_read(service: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-w"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// Writes the item as Claude Code does, through `security -i` so the secret isn't on a command line.
#[cfg(target_os = "macos")]
fn keychain_write(service: &str, secret: &str) -> Result<(), String> {
    use std::io::Write;
    let hex: String = secret.bytes().map(|b| format!("{b:02x}")).collect();
    let mut child = std::process::Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    writeln!(child.stdin.take().ok_or("no stdin")?, "add-generic-password -U -a \"{}\" -s \"{service}\" -X {hex}", user()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(format!("Keychain: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// This machine's sign-in for an account.
pub fn read(id: &str) -> Result<Parts, String> {
    let dir = accounts::dir(id).ok_or_else(|| format!("unknown account {id}"))?;
    let mut parts = Parts::new();
    match accounts::provider_of(id) {
        "claude" => {
            #[cfg(target_os = "macos")]
            let keychain = keychain_read(&claude_service(id, &dir));
            #[cfg(not(target_os = "macos"))]
            let keychain: Option<String> = None;
            let creds = keychain.or_else(|| std::fs::read_to_string(dir.join(".credentials.json")).ok()).ok_or("Not signed in to Claude Code on this account.")?;
            serde_json::from_str::<Value>(&creds).map_err(|_| "Claude Code's sign-in isn't readable.")?;
            parts.insert("credentials".into(), creds);
            let record = claude_json(id).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
            if let Some(account) = record.map(|r| r["oauthAccount"].clone()).filter(|a| a.is_object()) {
                parts.insert("account".into(), account.to_string());
            }
        }
        "codex" => {
            let auth = std::fs::read_to_string(dir.join("auth.json")).map_err(|_| "Not signed in to Codex on this account.")?;
            parts.insert("auth".into(), auth);
        }
        other => return Err(format!("{other} sign-ins can't be shared")),
    }
    Ok(parts)
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("winfer-tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Puts a sign-in in place for an account on this machine.
pub fn write(id: &str, parts: &Parts) -> Result<(), String> {
    let dir = accounts::dir(id).ok_or_else(|| format!("unknown account {id}"))?;
    match accounts::provider_of(id) {
        "claude" => {
            let creds = parts.get("credentials").ok_or("no Claude sign-in in it")?;
            serde_json::from_str::<Value>(creds).map_err(|_| "a broken Claude sign-in")?;
            #[cfg(target_os = "macos")]
            keychain_write(&claude_service(id, &dir), creds)?;
            #[cfg(not(target_os = "macos"))]
            write_private(&dir.join(".credentials.json"), creds)?;
            // Claude Code's record: signed in as that account, its first-run setup done.
            if let Some(path) = claude_json(id) {
                let mut record: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
                if let Some(account) = parts.get("account").and_then(|a| serde_json::from_str::<Value>(a).ok()) {
                    record["oauthAccount"] = account;
                }
                record["hasCompletedOnboarding"] = json!(true);
                if record.get("theme").is_none() {
                    record["theme"] = json!("dark");
                }
                write_private(&path, &serde_json::to_string_pretty(&record).unwrap_or_default())?;
            }
        }
        "codex" => write_private(&dir.join("auth.json"), parts.get("auth").ok_or("no Codex sign-in in it")?)?,
        other => return Err(format!("{other} sign-ins can't be shared")),
    }
    Ok(())
}

/// The account a pushed sign-in goes to here: the agent's default one while it has no sign-in,
/// else a new account (so failover can use both).
fn target_for(provider: &str, label: &str) -> Result<String, String> {
    let list = accounts::accounts_list();
    let default = accounts::default_id(provider);
    if !list.iter().any(|e| e.account.id == default && e.signed_in) && read(&default).is_err() {
        return Ok(default);
    }
    accounts::accounts_add(provider.to_string(), label.to_string()).map(|a| a.id)
}

/// `account.push` from a paired machine: puts the sign-in here and links the two. The reply's
/// `accountId` is this machine's account.
pub fn on_push(peer: &str, msg: &Value) -> Value {
    let provider = msg["provider"].as_str().unwrap_or_default();
    let remote = msg["accountId"].as_str().unwrap_or_default().to_string();
    let label = msg["label"].as_str().unwrap_or("Pushed").to_string();
    let parts: Parts = serde_json::from_value(msg["parts"].clone()).unwrap_or_default();
    if !accounts::PROVIDERS.contains(&provider) || parts.is_empty() {
        return json!({"t": "ack", "ok": false, "error": "That sign-in can't be used here."});
    }
    // Pushed again: the same account here.
    let existing = with_links(|l| l.iter().find(|x| x.peer == peer && x.remote == remote).map(|x| x.local.clone()));
    let local = match existing.map(Ok).unwrap_or_else(|| target_for(provider, &label)) {
        Ok(id) => id,
        Err(e) => return json!({"t": "ack", "ok": false, "error": e}),
    };
    if let Err(e) = write(&local, &parts) {
        return json!({"t": "ack", "ok": false, "error": e});
    }
    let fp = fingerprint(&parts);
    with_links(|l| {
        l.retain(|x| !(x.peer == peer && x.remote == remote));
        l.push(Link { peer: peer.into(), local: local.clone(), remote, provider: provider.into(), fp, received: true });
    });
    json!({"t": "ack", "ok": true, "accountId": local, "label": accounts::label(&local)})
}

/// The `account.push` message for one of this machine's accounts.
pub fn push_message(id: &str) -> Result<(Value, String), String> {
    let parts = read(id)?;
    let fp = fingerprint(&parts);
    Ok((json!({"t": "account.push", "provider": accounts::provider_of(id), "accountId": id, "label": accounts::label(id), "parts": parts}), fp))
}

/// The other machine took a push: link the two.
pub fn pushed(peer: &str, local: &str, remote: &str, fp: &str) {
    with_links(|l| {
        l.retain(|x| !(x.peer == peer && x.local == local));
        l.push(Link { peer: peer.into(), local: local.into(), remote: remote.into(), provider: accounts::provider_of(local).into(), fp: fp.into(), received: false });
    });
}

/// `account.sync`: the other machine refreshed a linked sign-in.
pub fn on_sync(peer: &str, msg: &Value) {
    let id = msg["accountId"].as_str().unwrap_or_default();
    let parts: Parts = serde_json::from_value(msg["parts"].clone()).unwrap_or_default();
    let Some(link) = links().into_iter().find(|x| x.peer == peer && x.local == id) else { return };
    let fp = fingerprint(&parts);
    if fp == link.fp {
        return;
    }
    match write(id, &parts) {
        Ok(()) => with_links(|l| {
            if let Some(x) = l.iter_mut().find(|x| x.peer == peer && x.local == id) {
                x.fp = fp;
            }
        }),
        Err(e) => eprintln!("account sync {id}: {e}"),
    }
}

/// Every half minute or so: linked sign-ins that changed here (a refresh) go to the other side.
/// `send` returns false when the machine isn't reachable (tried again next time).
pub fn tick(send: impl Fn(&str, Value) -> bool) {
    for link in links() {
        let Ok(parts) = read(&link.local) else { continue };
        let fp = fingerprint(&parts);
        if fp != link.fp && send(&link.peer, json!({"t": "account.sync", "accountId": link.remote, "parts": parts})) {
            with_links(|l| {
                if let Some(x) = l.iter_mut().find(|x| x.peer == link.peer && x.local == link.local) {
                    x.fp = fp.clone();
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_follow_the_content() {
        let mut a = Parts::new();
        a.insert("auth".into(), "{\"x\":1}".into());
        let mut b = a.clone();
        assert_eq!(fingerprint(&a), fingerprint(&b));
        b.insert("auth".into(), "{\"x\":2}".into());
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }
}
