//! Moving winfer to another machine: export the settings, workspaces, plugins and usage history
//! into one `.winfer` file (a zip) and import it on the other side.
//!
//! Secrets (ChatGPT sign-in, the voice API key, plugin vault values) are only exported when the
//! user gives a passphrase, encrypted with it. Agent sign-ins, paired phones, browser profiles,
//! worktrees and `winfer code` chats stay on the machine.
//!
//! Importing replaces the current data. Files can't be swapped under the open databases, so the
//! import is unpacked into `import-pending/` and applied on the next start, before anything is
//! opened ([`apply_pending`]); the previous files are kept in `pre-import-<time>/`.

use std::fs::File;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::path::Path;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Manager};
use zip::write::SimpleFileOptions;

use crate::vault;

/// Bumped when the archive layout changes in a way older builds can't read.
const FORMAT: u32 = 1;
const PENDING: &str = "import-pending";
/// Written last when unpacking, so a half-unpacked import is never applied.
const READY: &str = ".ready";
/// The imported `localStorage`, until the webview picks it up.
const LOCAL_STORAGE: &str = "import-local-storage.json";
const MANIFEST: &str = "manifest.json";
const SECRETS: &str = "secrets.enc";
/// Files and folders (relative to the data dir) an export carries over as they are.
const COPIED: &[&str] = &["harness/config.json", "plugins.json", "plugins", "plugin-data"];
/// What an import replaces; moved to the backup first.
const REPLACED: &[&str] = &[
    "winfer.db",
    "winfer.db-wal",
    "winfer.db-shm",
    "usage.db",
    "usage.db-wal",
    "usage.db-shm",
    "accounts.json",
    "harness/config.json",
    "plugins.json",
    "plugins",
    "plugin-data",
];
#[cfg(not(test))]
const ITERATIONS: u32 = 600_000;
#[cfg(test)]
const ITERATIONS: u32 = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    pub app_version: String,
    /// Unix milliseconds.
    pub created_at: u64,
    /// The exporting machine's home folder, to move paths under it to this machine's.
    pub home: String,
    pub platform: String,
    pub has_secrets: bool,
    /// Hex; set when `has_secrets`.
    #[serde(default)]
    pub salt: Option<String>,
}

/// A keychain item: (service, account, value).
type Secret = (String, String, String);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn home() -> String {
    dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default()
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// --- Secrets ---

fn derive_key(passphrase: &str, salt: &[u8]) -> LessSafeKey {
    let mut key = [0u8; 32];
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ITERATIONS).unwrap(), salt, passphrase.as_bytes(), &mut key);
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap())
}

/// Nonce followed by the sealed data.
fn encrypt(plain: &[u8], passphrase: &str, salt: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(|_| "No randomness available")?;
    let mut data = plain.to_vec();
    derive_key(passphrase, salt)
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data)
        .map_err(|_| "Encryption failed")?;
    Ok([nonce.as_slice(), &data].concat())
}

fn decrypt(sealed: &[u8], passphrase: &str, salt: &[u8]) -> Result<Vec<u8>, String> {
    let wrong = || "Wrong passphrase".to_string();
    if sealed.len() < NONCE_LEN {
        return Err(wrong());
    }
    let (nonce, data) = sealed.split_at(NONCE_LEN);
    let mut data = data.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| wrong())?;
    let plain = derive_key(passphrase, salt).open_in_place(nonce, Aad::empty(), &mut data).map_err(|_| wrong())?;
    Ok(plain.to_vec())
}

/// The keychain items worth moving: the ChatGPT sign-in, the voice key and every plugin's vault.
fn collect_secrets(data_dir: &Path) -> Vec<Secret> {
    let mut wanted = vec![
        ("dev.winfer.app".to_string(), "chatgpt-account".to_string()),
        ("dev.winfer.app.voice".to_string(), "OPENAI_API_KEY".to_string()),
    ];
    if let Ok(plugins) = std::fs::read_dir(data_dir.join("plugin-data")) {
        for plugin in plugins.flatten() {
            let index: Value = std::fs::read_to_string(plugin.path().join("vault.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            let id = plugin.file_name().to_string_lossy().into_owned();
            for key in index["secrets"].as_object().into_iter().flat_map(|m| m.keys()) {
                wanted.push((vault::service(&id), key.clone()));
            }
        }
    }
    wanted.into_iter().filter_map(|(service, key)| vault::values::get(&service, &key).ok().map(|v| (service, key, v))).collect()
}

// --- Export ---

fn add_file(zip: &mut zip::ZipWriter<File>, name: &str, data: &[u8]) -> Result<(), String> {
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(data.len() > u32::MAX as usize / 2);
    zip.start_file(name, options).map_err(err)?;
    zip.write_all(data).map_err(err)
}

/// Adds a file or folder under the data dir, by its relative path. Symlinks are skipped: they
/// point at things on this machine.
fn add_path(zip: &mut zip::ZipWriter<File>, data_dir: &Path, rel: &Path) -> Result<(), String> {
    let path = data_dir.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return Ok(()) };
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&path).map_err(err)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            add_path(zip, data_dir, &rel.join(entry.file_name()))?;
        }
        return Ok(());
    }
    let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
    add_file(zip, &name, &std::fs::read(&path).map_err(err)?)
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn export(
    data_dir: &Path,
    dest: &Path,
    app_version: &str,
    dbs: impl FnOnce(&Path) -> Result<(), String>,
    local_storage: &Map<String, Value>,
    passphrase: Option<&str>,
) -> Result<(), String> {
    let tmp = data_dir.join(format!(".export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(err)?;
    let result = (|| {
        dbs(&tmp)?;
        let usage = tmp.join("usage.db");
        if usage.exists() {
            // How far this machine's agent logs were read means nothing on another one.
            Connection::open(&usage).and_then(|c| c.execute_batch("DELETE FROM files; DELETE FROM kv;")).map_err(err)?;
        }

        let salt = passphrase.map(|_| {
            let mut salt = [0u8; 16];
            let _ = SystemRandom::new().fill(&mut salt);
            salt
        });
        let manifest = Manifest {
            format: FORMAT,
            app_version: app_version.into(),
            created_at: now_ms(),
            home: home(),
            platform: std::env::consts::OS.into(),
            has_secrets: salt.is_some(),
            salt: salt.map(hex::encode),
        };

        let mut zip = zip::ZipWriter::new(File::create(dest).map_err(err)?);
        add_file(&mut zip, MANIFEST, &serde_json::to_vec_pretty(&manifest).map_err(err)?)?;
        for db in ["winfer.db", "usage.db"] {
            if let Ok(data) = std::fs::read(tmp.join(db)) {
                add_file(&mut zip, db, &data)?;
            }
        }
        if let Some(file) = read_json(&data_dir.join("accounts.json")) {
            // The list only: which accounts are out of usage is this machine's business.
            add_file(&mut zip, "accounts.json", &serde_json::to_vec_pretty(&json!({ "accounts": file["accounts"] })).map_err(err)?)?;
        }
        if let Some(url) = read_json(&data_dir.join("mobile.json")).and_then(|m| m["relayUrl"].as_str().map(String::from)) {
            add_file(&mut zip, "mobile.json", &serde_json::to_vec(&json!({ "relayUrl": url })).map_err(err)?)?;
        }
        for rel in COPIED {
            add_path(&mut zip, data_dir, Path::new(rel))?;
        }
        add_file(&mut zip, "local-storage.json", &serde_json::to_vec(local_storage).map_err(err)?)?;
        if let (Some(passphrase), Some(salt)) = (passphrase, salt) {
            let secrets = serde_json::to_vec(&collect_secrets(data_dir)).map_err(err)?;
            add_file(&mut zip, SECRETS, &encrypt(&secrets, passphrase, &salt)?)?;
        }
        zip.finish().map_err(err)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

/// Writes the export to `path`. `local_storage` is the webview's, which only it can read.
#[tauri::command]
pub async fn data_export(app: AppHandle, path: String, local_storage: Map<String, Value>, passphrase: Option<String>) -> Result<(), String> {
    let passphrase = passphrase.filter(|p| !p.is_empty());
    tauri::async_runtime::spawn_blocking(move || {
        let data_dir = app.path().app_data_dir().map_err(err)?;
        let version = app.package_info().version.to_string();
        let dbs = |tmp: &Path| {
            app.state::<crate::store::Store>().vacuum_into(&tmp.join("winfer.db")).map_err(err)?;
            app.state::<crate::usage_history::UsageHistory>().vacuum_into(&tmp.join("usage.db")).map_err(err)
        };
        export(&data_dir, Path::new(&path), &version, dbs, &local_storage, passphrase.as_deref())
    })
    .await
    .map_err(err)?
}

// --- Import ---

fn open_archive(path: &Path) -> Result<zip::ZipArchive<File>, String> {
    zip::ZipArchive::new(File::open(path).map_err(err)?).map_err(|_| "Not a winfer export".to_string())
}

fn read_entry(archive: &mut zip::ZipArchive<File>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    Some(data)
}

fn read_manifest(archive: &mut zip::ZipArchive<File>) -> Result<Manifest, String> {
    let manifest: Manifest = read_entry(archive, MANIFEST).and_then(|d| serde_json::from_slice(&d).ok()).ok_or("Not a winfer export")?;
    if manifest.format > FORMAT {
        return Err(format!("This export is from a newer winfer ({}); update winfer to import it", manifest.app_version));
    }
    Ok(manifest)
}

/// What the import dialog shows before the user confirms.
#[tauri::command]
pub fn data_import_inspect(path: String) -> Result<Manifest, String> {
    read_manifest(&mut open_archive(Path::new(&path))?)
}

/// Unpacks the export for the next start and stores its secrets (when `passphrase` is given)
/// in the keychain right away, so they're never on disk unencrypted.
fn stage(data_dir: &Path, path: &Path, passphrase: Option<&str>) -> Result<(), String> {
    let mut archive = open_archive(path)?;
    let manifest = read_manifest(&mut archive)?;
    let secrets: Vec<Secret> = match (passphrase, manifest.has_secrets) {
        (Some(passphrase), true) => {
            let salt = hex::decode(manifest.salt.as_deref().unwrap_or_default()).map_err(err)?;
            let sealed = read_entry(&mut archive, SECRETS).ok_or("The export's secrets are missing")?;
            serde_json::from_slice(&decrypt(&sealed, passphrase, &salt)?).map_err(err)?
        }
        _ => Vec::new(),
    };

    let pending = data_dir.join(PENDING);
    let _ = std::fs::remove_dir_all(&pending);
    std::fs::create_dir_all(&pending).map_err(err)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(err)?;
        // `enclosed_name` rejects names that would land outside the folder.
        let Some(rel) = file.enclosed_name() else { continue };
        if file.is_dir() || rel == Path::new(SECRETS) {
            continue;
        }
        let out = pending.join(rel);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        std::io::copy(&mut file, &mut File::create(&out).map_err(err)?).map_err(err)?;
    }
    for (service, key, value) in &secrets {
        vault::values::set(service, key, value)?;
    }
    std::fs::write(pending.join(READY), b"").map_err(err)
}

#[tauri::command]
pub async fn data_import_stage(app: AppHandle, path: String, passphrase: Option<String>) -> Result<(), String> {
    let passphrase = passphrase.filter(|p| !p.is_empty());
    let data_dir = app.path().app_data_dir().map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || stage(&data_dir, Path::new(&path), passphrase.as_deref())).await.map_err(err)?
}

/// The imported `localStorage`, once; the webview writes it back before the app loads.
#[tauri::command]
pub fn data_import_take_local(app: AppHandle) -> Option<Map<String, Value>> {
    let path = app.path().app_data_dir().ok()?.join(LOCAL_STORAGE);
    let data = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&data).ok()
}

/// `s` with `from` (a folder) replaced by `to` wherever it's a whole path prefix, or `None`
/// when nothing changed.
fn remap(s: &str, from: &str, to: &str) -> Option<String> {
    if from.is_empty() || from == to || !s.contains(from) {
        return None;
    }
    let (mut out, mut rest, mut changed) = (String::with_capacity(s.len()), s, false);
    while let Some(i) = rest.find(from) {
        let after = &rest[i + from.len()..];
        let whole = after.is_empty() || after.starts_with(['/', '\\', '"']);
        out.push_str(&rest[..i]);
        out.push_str(if whole { to } else { from });
        changed |= whole;
        rest = after;
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// [`remap`] inside JSON text, where the paths are escaped (Windows' backslashes doubled).
fn remap_json(s: &str, from: &str, to: &str) -> Option<String> {
    remap(s, &escape_json(from), &escape_json(to))
}

/// `p` as it appears inside a JSON string.
fn escape_json(p: &str) -> String {
    let quoted = serde_json::to_string(p).unwrap_or_default();
    quoted[1..quoted.len() - 1].to_string()
}

/// `json` when the column holds JSON text.
fn remap_column(conn: &Connection, table: &str, column: &str, json: bool, from: &str, to: &str) -> rusqlite::Result<()> {
    let (from, to) = if json { (escape_json(from), escape_json(to)) } else { (from.to_string(), to.to_string()) };
    let (from, to) = (from.as_str(), to.as_str());
    let rows: Vec<(i64, String)> = conn
        .prepare(&format!("SELECT rowid, {column} FROM {table} WHERE instr({column}, ?1) > 0"))?
        .query_map([from], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut update = conn.prepare(&format!("UPDATE {table} SET {column} = ?1 WHERE rowid = ?2"))?;
    for (rowid, value) in rows {
        if let Some(value) = remap(&value, from, to) {
            update.execute(rusqlite::params![value, rowid])?;
        }
    }
    Ok(())
}

/// Moves paths to this machine's home and drops worktrees that didn't come along (panes in
/// them go back to the repo).
fn fix_paths(data_dir: &Path, from: &str, to: &str) -> Result<(), String> {
    let db = data_dir.join("winfer.db");
    if db.exists() {
        let conn = Connection::open(&db).map_err(err)?;
        for (table, column, json) in [("workspaces", "root_dir", false), ("panes", "cwd", false), ("panes", "worktree", true), ("merges", "repo", false), ("kv", "value", true)] {
            remap_column(&conn, table, column, json, from, to).map_err(err)?;
        }
        let worktrees: Vec<(String, String)> = conn
            .prepare("SELECT id, worktree FROM panes WHERE worktree IS NOT NULL")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .map_err(err)?;
        for (id, worktree) in worktrees {
            let worktree: Value = serde_json::from_str(&worktree).unwrap_or_default();
            if worktree["path"].as_str().is_some_and(|p| Path::new(p).exists()) {
                continue;
            }
            conn.execute("UPDATE panes SET worktree = NULL, cwd = coalesce(?1, cwd) WHERE id = ?2", rusqlite::params![worktree["repo"].as_str(), id]).map_err(err)?;
        }
    }
    let usage = data_dir.join("usage.db");
    if usage.exists() {
        remap_column(&Connection::open(&usage).map_err(err)?, "events", "cwd", false, from, to).map_err(err)?;
    }
    let config = data_dir.join("harness/config.json");
    if let Some(text) = std::fs::read_to_string(&config).ok().and_then(|t| remap_json(&t, from, to)) {
        std::fs::write(&config, text).map_err(err)?;
    }
    Ok(())
}

/// Applies a staged import. Runs at start, before the databases are opened.
pub fn apply_pending(data_dir: &Path) -> Result<(), String> {
    let pending = data_dir.join(PENDING);
    if !pending.exists() {
        return Ok(());
    }
    if !pending.join(READY).exists() {
        return std::fs::remove_dir_all(&pending).map_err(err);
    }
    let manifest: Manifest = read_json(&pending.join(MANIFEST)).and_then(|m| serde_json::from_value(m).ok()).ok_or("The staged import has no manifest")?;

    let backup = data_dir.join(format!("pre-import-{}", now_ms()));
    for rel in REPLACED {
        let path = data_dir.join(rel);
        if std::fs::symlink_metadata(&path).is_ok() {
            let to = backup.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).map_err(err)?;
            std::fs::rename(&path, &to).map_err(err)?;
        }
        let staged = pending.join(rel);
        if staged.exists() {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
            std::fs::rename(&staged, &path).map_err(err)?;
        }
    }
    // Paired phones stay this machine's: only the relay setting comes over.
    if let Some(url) = read_json(&pending.join("mobile.json")).and_then(|m| m["relayUrl"].as_str().map(String::from)) {
        let path = data_dir.join("mobile.json");
        let mut mobile = read_json(&path).unwrap_or_else(|| json!({}));
        if path.exists() {
            std::fs::create_dir_all(&backup).map_err(err)?;
            std::fs::copy(&path, backup.join("mobile.json")).map_err(err)?;
        }
        mobile["relayUrl"] = json!(url);
        std::fs::write(&path, serde_json::to_vec_pretty(&mobile).map_err(err)?).map_err(err)?;
    }
    let local = pending.join("local-storage.json");
    if local.exists() {
        std::fs::rename(&local, data_dir.join(LOCAL_STORAGE)).map_err(err)?;
    }
    std::fs::remove_dir_all(&pending).map_err(err)?;

    fix_paths(data_dir, &manifest.home, &home())?;
    // Extra agent accounts need their folders here; they sign in again.
    if let Some(accounts) = read_json(&data_dir.join("accounts.json")) {
        for id in accounts["accounts"].as_array().into_iter().flatten().filter_map(|a| a["id"].as_str()) {
            let _ = crate::accounts::ensure_links(id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("winfer-transfer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn secrets_round_trip() {
        let salt = [7u8; 16];
        let sealed = encrypt(b"top secret", "pass", &salt).unwrap();
        assert_eq!(decrypt(&sealed, "pass", &salt).unwrap(), b"top secret");
        assert_eq!(decrypt(&sealed, "wrong", &salt).unwrap_err(), "Wrong passphrase");
    }

    #[test]
    fn remaps_whole_path_prefixes() {
        assert_eq!(remap("/Users/a/code", "/Users/a", "/home/b").as_deref(), Some("/home/b/code"));
        assert_eq!(remap(r#"{"x":"/Users/a"}"#, "/Users/a", "/home/b").as_deref(), Some(r#"{"x":"/home/b"}"#));
        assert_eq!(remap("/Users/ab/code", "/Users/a", "/home/b"), None);
        assert_eq!(remap("/Users/a/x", "/Users/a", "/Users/a"), None);
        // Windows homes have backslashes, which JSON escapes.
        let json = remap_json(r#"{"x":"/Users/a/p"}"#, "/Users/a", r"C:\Users\b").unwrap();
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap()["x"], r"C:\Users\b/p");
        let back = remap_json(&json, r"C:\Users\b", "/Users/a").unwrap();
        assert_eq!(back, r#"{"x":"/Users/a/p"}"#);
    }

    #[test]
    fn export_then_import() {
        let from = temp("from");
        std::fs::create_dir_all(from.join("plugins/demo")).unwrap();
        std::fs::write(from.join("plugins/demo/plugin.json"), "{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", from.join("plugins/demo/link")).unwrap();
        std::fs::create_dir_all(from.join("plugin-data/demo")).unwrap();
        std::fs::write(from.join("plugin-data/demo/vault.json"), r#"{"secrets":{"TOKEN":{"key":"TOKEN"}}}"#).unwrap();
        vault::values::set(&vault::service("demo"), "TOKEN", "s3cret").unwrap();
        std::fs::write(from.join("accounts.json"), r#"{"accounts":[{"id":"claude:default","provider":"claude","label":"Default"}],"exhausted":{"claude:default":5}}"#).unwrap();

        let old_home = "/Users/olduser";
        let dbs = |tmp: &Path| {
            let store = crate::store::Store::open(&from.join("winfer.db")).map_err(err)?;
            let conn = Connection::open(from.join("winfer.db")).unwrap();
            conn.execute("INSERT INTO workspaces (id, name, root_dir, sort) VALUES ('w', 'W', ?1, 0)", [format!("{old_home}/proj")]).unwrap();
            conn.execute(
                "INSERT INTO panes (id, workspace_id, provider, title, cwd, worktree, sort) VALUES ('p', 'w', 'claude', 't', ?1, ?2, 0)",
                [format!("{old_home}/wt"), json!({ "path": format!("{old_home}/wt"), "repo": format!("{old_home}/proj") }).to_string()],
            )
            .unwrap();
            store.vacuum_into(&tmp.join("winfer.db")).map_err(err)
        };
        let archive = from.join("out.winfer");
        let mut local = Map::new();
        local.insert("winfer.theme".into(), json!("dark"));
        export(&from, &archive, "1.0.0", dbs, &local, Some("pw")).unwrap();

        // Pretend the export came from another home.
        let mut zip = open_archive(&archive).unwrap();
        let mut manifest = read_manifest(&mut zip).unwrap();
        assert!(manifest.has_secrets);
        assert!(zip.by_name("plugins/demo/plugin.json").is_ok());
        assert!(zip.by_name("plugins/demo/link").is_err());
        assert!(zip.by_name("winfer.db").is_ok());
        manifest.home = old_home.into();
        drop(zip);
        let rewritten = from.join("rewritten.winfer");
        {
            let mut src = open_archive(&archive).unwrap();
            let mut out = zip::ZipWriter::new(File::create(&rewritten).unwrap());
            add_file(&mut out, MANIFEST, &serde_json::to_vec(&manifest).unwrap()).unwrap();
            for i in 0..src.len() {
                let file = src.by_index(i).unwrap();
                if file.name().is_ok_and(|n| n != MANIFEST) {
                    out.raw_copy_file(file).unwrap();
                }
            }
            out.finish().unwrap();
        }

        let to = temp("to");
        assert_eq!(stage(&to, &rewritten, Some("nope")).unwrap_err(), "Wrong passphrase");
        vault::values::delete(&vault::service("demo"), "TOKEN").unwrap();
        stage(&to, &rewritten, Some("pw")).unwrap();
        assert_eq!(vault::values::get(&vault::service("demo"), "TOKEN").unwrap(), "s3cret");
        apply_pending(&to).unwrap();

        assert!(!to.join(PENDING).exists());
        assert!(to.join("plugins/demo/plugin.json").exists());
        assert!(to.join(LOCAL_STORAGE).exists());
        let accounts = read_json(&to.join("accounts.json")).unwrap();
        assert!(accounts.get("exhausted").is_none());
        let conn = Connection::open(to.join("winfer.db")).unwrap();
        let root: String = conn.query_row("SELECT root_dir FROM workspaces", [], |r| r.get(0)).unwrap();
        assert_eq!(root, format!("{}/proj", home()));
        let (cwd, worktree): (String, Option<String>) = conn.query_row("SELECT cwd, worktree FROM panes", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(cwd, format!("{}/proj", home()));
        assert_eq!(worktree, None);

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }
}
