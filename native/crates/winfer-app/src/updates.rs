//! App updates and release notes (port of `features/updates/releaseNotes.ts` and `updater.ts`).
//! Native builds ship in the Tauri app's GitHub releases (vX.Y.Z stable, vX.Y.Z-N Nebula; see
//! .github/workflows/release.yml) with an update feed of their own, latest-native.json, and are
//! signed with the Tauri app's updater key. Installing downloads this platform's bundle, checks its
//! signature, puts it in place of the running app and relaunches into it. On Windows putting it in
//! place needs administrator permission, which a UAC prompt asks for.

use base64::Engine as _;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "RickyDane/winfer";
pub const TAG_PREFIX: &str = "v";
/// The native builds' feed, next to the Tauri app's latest.json on the same releases.
pub const FEED_FILE: &str = "latest-native.json";
/// Set on the copy started after an update, which waits for this one to quit (main.rs).
pub const RELAUNCH_ENV: &str = "WINFER_RELAUNCHED";
/// Most an update bundle may weigh.
const MAX_BUNDLE: u64 = 1 << 30;
/// The argument that has a copy started as administrator apply a downloaded update (main.rs).
#[cfg(windows)]
pub const APPLY_ARG: &str = "apply-update";

/// One release's changes, as the "What's new" dialog lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseChanges {
    pub version: String,
    pub changes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nebula,
}

impl Channel {
    pub fn id(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nebula => "nebula",
        }
    }

    pub fn parse(id: &str) -> Option<Channel> {
        match id {
            "stable" => Some(Channel::Stable),
            "nebula" => Some(Channel::Nebula),
            _ => None,
        }
    }

    /// The channel of a build, from its version (Nebula builds are versioned X.Y.Z-N).
    pub fn of_version(version: &str) -> Channel {
        if version.contains('-') { Channel::Nebula } else { Channel::Stable }
    }

    /// The feed on the latest (stable) release, or on the release each Nebula build replaces.
    pub fn feed(self) -> String {
        match self {
            Channel::Stable => format!("https://github.com/{REPO}/releases/latest/download/{FEED_FILE}"),
            Channel::Nebula => format!("https://github.com/{REPO}/releases/download/nebula-channel/{FEED_FILE}"),
        }
    }
}

/// Release notes as the release workflow writes them: a "### Changes" list of commit subjects, then
/// a "---" and the install footer. Only the changes are worth showing in the app.
pub fn parse_changes(body: Option<&str>) -> Vec<String> {
    let Some(body) = body else { return Vec::new() };
    let lines: Vec<&str> = body.lines().collect();
    let start = lines.iter().position(|l| {
        let t = l.trim();
        t.starts_with('#') && t.trim_start_matches('#').trim().eq_ignore_ascii_case("changes")
    });
    let mut changes = Vec::new();
    for line in lines.iter().skip(start.map_or(0, |s| s + 1)) {
        let t = line.trim();
        if t == "---" || (start.is_some() && t.starts_with('#')) {
            break;
        }
        if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            if !item.trim().is_empty() {
                changes.push(item.trim().to_string());
            }
        }
    }
    changes
}

/// "Plugins: add Telegram" → (Some("Plugins"), "add Telegram"); no prefix → no area.
pub fn split_area(change: &str) -> (Option<String>, String) {
    if let Some((head, rest)) = change.split_once(": ") {
        let mut chars = head.chars();
        let ok = head.len() <= 31 && chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_alphanumeric() || " ./+-_".contains(c)) && !rest.trim().is_empty();
        if ok {
            return (Some(head.to_string()), rest.trim().to_string());
        }
    }
    (None, change.to_string())
}

pub use winfer_core::version::compare_versions;

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

impl Release {
    /// The version of a vX.Y.Z or vX.Y.Z-N release, or None for the repo's other releases (the
    /// Nebula feed's).
    pub fn version(&self) -> Option<String> {
        let v = self.tag_name.strip_prefix(TAG_PREFIX)?;
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
        let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        (core.split('.').count() == 3 && core.split('.').all(number) && pre.is_none_or(number)).then(|| v.to_string())
    }
}

/// The releases after `from` up to and including `to`, newest first, so skipping versions still
/// shows everything that changed. No `from` → just `to`. Nebula (pre-release) builds only count
/// when updating to one; a stable release's notes already cover everything since the last stable.
pub fn releases_between(releases: &[Release], from: Option<&str>, to: &str) -> Vec<ReleaseChanges> {
    use std::cmp::Ordering::*;
    let nebula = to.contains('-');
    let mut out: Vec<ReleaseChanges> = releases
        .iter()
        .filter(|r| !r.draft && (nebula || !r.prerelease))
        .filter_map(|r| Some(ReleaseChanges { version: r.version()?, changes: parse_changes(r.body.as_deref()) }))
        .filter(|r| compare_versions(&r.version, to) != Greater && match from {
            Some(f) => compare_versions(&r.version, f) == Greater,
            None => compare_versions(&r.version, to) == Equal,
        })
        .collect();
    out.sort_by(|a, b| compare_versions(&b.version, &a.version));
    out
}

/// An update feed as release.yml writes it (the shape of the Tauri app's latest.json).
#[derive(Clone, Debug, Deserialize)]
pub struct Feed {
    pub version: String,
    #[serde(default)]
    pub platforms: HashMap<String, Bundle>,
}

/// One platform's update: a .app.tar.gz (macOS), the .exe (Windows) or a .tar.gz of the binary
/// (Linux), and its minisign signature.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Bundle {
    pub url: String,
    pub signature: String,
}

/// This build's key in a feed's platforms: darwin-aarch64, windows-x86_64, linux-x86_64, …
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// The bundle `feed` offers this build: only for a newer version (so never a downgrade: a Nebula
/// build switched to stable waits for the stable release of its version), and only if it has one
/// for this platform.
pub fn offered(feed: &Feed, current: &str) -> Option<Bundle> {
    if compare_versions(&feed.version, current) != std::cmp::Ordering::Greater {
        return None;
    }
    feed.platforms.get(&platform()).cloned()
}

/// The release page of a version, for downloading it by hand.
pub fn release_page(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/{TAG_PREFIX}{version}")
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).build().into()
}

/// The channel's feed (blocking). None while there's none: the latest stable release predates
/// native builds, or its native bundles are still building.
pub fn fetch_feed(channel: Channel) -> Result<Option<Feed>, String> {
    match agent(Duration::from_secs(15)).get(&channel.feed()).header("User-Agent", "winfer").call() {
        Ok(mut res) => res.body_mut().read_json::<Feed>().map(Some).map_err(|e| e.to_string()),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(format!("GitHub answered: {e}")),
    }
}

/// All releases of the repo (blocking: call off the UI thread).
pub fn fetch_releases() -> Result<Vec<Release>, String> {
    let mut res = agent(Duration::from_secs(15))
        .get(&format!("https://api.github.com/repos/{REPO}/releases?per_page=100"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "winfer")
        .call()
        .map_err(|e| format!("GitHub answered: {e}"))?;
    res.body_mut().read_json::<Vec<Release>>().map_err(|e| e.to_string())
}

/// The Tauri app's updater key (plugins.updater.pubkey), which signs the native bundles too.
fn updater_pubkey() -> String {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../../../../src-tauri/tauri.conf.json")).unwrap_or_default();
    conf["plugins"]["updater"]["pubkey"].as_str().unwrap_or_default().to_string()
}

/// Checks `data` against a signature from `tauri signer sign`, as the Tauri updater does: the key
/// and the signature are base64 of minisign's text files.
pub fn verify(data: &[u8], signature: &str, pubkey: &str) -> Result<(), String> {
    let text = |b64: &str| base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok().and_then(|b| String::from_utf8(b).ok());
    let key = text(pubkey).and_then(|k| minisign_verify::PublicKey::decode(&k).ok()).ok_or("the update key is invalid")?;
    let sig = text(signature).and_then(|s| minisign_verify::Signature::decode(&s).ok()).ok_or("the update's signature is invalid")?;
    key.verify(data, &sig, true).map_err(|_| "the download doesn't match its signature".to_string())
}

/// What an update replaces: the .app bundle on macOS, else the executable.
fn install_target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if cfg!(target_os = "macos") {
        // …/winfer.app/Contents/MacOS/winfer
        let exe = exe.canonicalize().map_err(|e| e.to_string())?;
        let app = exe.ancestors().nth(3).filter(|a| a.extension().is_some_and(|e| e == "app"));
        return app.map(Path::to_path_buf).ok_or_else(|| "winfer isn't running from its app bundle".to_string());
    }
    Ok(exe)
}

/// Where an install unpacks: beside what it replaces, so moving it into place is a rename.
fn staging_dir(target: &Path) -> Option<PathBuf> {
    Some(target.parent()?.join(format!(".winfer-update-{}", std::process::id())))
}

/// Downloads `bundle`, checks its signature and puts it in place of the running app, reporting
/// (bytes so far, total) as it downloads. Returns the executable to relaunch. Blocking.
pub fn install(bundle: &Bundle, progress: impl Fn(u64, Option<u64>)) -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        return Err("development builds don't update themselves".into());
    }
    let target = install_target()?;
    let staging = staging_dir(&target).ok_or("winfer's folder isn't known")?;
    std::fs::create_dir_all(&staging).map_err(|e| format!("can't write to {}: {e}", staging.parent().unwrap_or(&staging).display()))?;
    let result = download(&bundle.url, progress).and_then(|data| {
        verify(&data, &bundle.signature, &updater_pubkey())?;
        #[cfg(not(windows))]
        return swap_in(&target, &staging, &data);
        #[cfg(windows)]
        return swap_in_as_admin(&target, &staging, &data, &bundle.signature);
    });
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn download(url: &str, progress: impl Fn(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
        .build()
        .into();
    let mut res = agent.get(url).header("User-Agent", "winfer").call().map_err(|e| format!("download failed: {e}"))?;
    let total = res.body().content_length();
    let mut reader = res.body_mut().with_config().limit(MAX_BUNDLE).reader();
    let mut data = Vec::with_capacity(total.unwrap_or(0).min(MAX_BUNDLE) as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress(data.len() as u64, total);
    }
    Ok(data)
}

/// Moves `target` aside and `new` into its place, putting `target` back if that fails.
fn replace(target: &Path, new: &Path, aside: &Path) -> Result<(), String> {
    std::fs::rename(target, aside).map_err(|e| format!("can't move {} aside: {e}", target.display()))?;
    std::fs::rename(new, target).map_err(|e| {
        let _ = std::fs::rename(aside, target);
        format!("can't put the update in place: {e}")
    })
}

/// Unpacks the .tar.gz of winfer.app (macOS) or the winfer binary (Linux) over `target`.
#[cfg(not(windows))]
fn swap_in(target: &Path, staging: &Path, archive: &[u8]) -> Result<PathBuf, String> {
    let file = staging.join("update.tar.gz");
    let unpacked = staging.join("new");
    std::fs::write(&file, archive).and_then(|_| std::fs::create_dir_all(&unpacked)).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("tar").arg("-xzf").arg(&file).arg("-C").arg(&unpacked).output().map_err(|e| format!("can't run tar: {e}"))?;
    if !out.status.success() {
        return Err(format!("can't unpack the update: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    if cfg!(target_os = "macos") {
        let app = std::fs::read_dir(&unpacked).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "app")).ok_or("the update has no app in it")?;
        replace(target, &app, &staging.join("old.app"))?;
        // The bundle's executable, under whatever name this copy was started as.
        let name = std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| n.to_os_string())).unwrap_or_else(|| "winfer".into());
        let exe = target.join("Contents/MacOS").join(&name);
        Ok(if exe.exists() { exe } else { target.join("Contents/MacOS/winfer") })
    } else {
        use std::os::unix::fs::PermissionsExt;
        let binary = unpacked.join("winfer");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("the update has no winfer binary in it: {e}"))?;
        replace(target, &binary, &staging.join("old"))?;
        Ok(target.to_path_buf())
    }
}

/// A file beside the .exe: `.new` while it's being written, `.old` where the replaced one waits to
/// be deleted.
#[cfg(windows)]
fn beside(exe: &Path, extension: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(extension);
    PathBuf::from(name)
}

/// The bundle is the new .exe. Replacing the installed one takes administrator permission: this
/// starts the running .exe again through a UAC prompt (APPLY_ARG, `apply_update`) and waits for it.
#[cfg(windows)]
fn swap_in_as_admin(target: &Path, staging: &Path, exe: &[u8], signature: &str) -> Result<PathBuf, String> {
    let download = staging.join("download.exe");
    std::fs::write(&download, exe).map_err(|e| e.to_string())?;
    // Quoted for the path's spaces (a Windows path has no quotes); the signature is base64.
    let args = format!("{APPLY_ARG} \"{}\" {}", download.display(), signature.trim());
    match run_as_admin(target, &args)? {
        0 => Ok(target.to_path_buf()),
        _ => Err("couldn't replace winfer.exe, even as administrator".into()),
    }
}

/// The administrator's side of an update: replaces this .exe, and only this one, with `download`.
/// Other processes can write to the download's folder, so the signature is checked again, on the
/// very bytes that get installed. Windows can't delete or overwrite a running .exe but can rename
/// it, so the old one moves to .old and is deleted on the next start (`cleanup`).
#[cfg(windows)]
pub fn apply_update(download: &Path, signature: &str) -> Result<(), String> {
    use std::io::Write as _;
    let data = std::fs::read(download).map_err(|e| e.to_string())?;
    verify(&data, signature, &updater_pubkey())?;
    let target = install_target()?;
    let (new, old) = (beside(&target, ".new"), beside(&target, ".old"));
    let _ = std::fs::remove_file(&new);
    // create_new: never writes through something already there.
    std::fs::File::create_new(&new).and_then(|mut f| f.write_all(&data).and_then(|_| f.sync_all())).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    replace(&target, &new, &old)
}

/// Runs `exe` with `args` as administrator and returns its exit code. Blocks while the UAC prompt
/// is up and the process runs.
#[cfg(windows)]
fn run_as_admin(exe: &Path, args: &str) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let file: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let args: Vec<u16> = args.encode_utf16().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points to outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        if e.code() == ERROR_CANCELLED.to_hresult() {
            "the update needs administrator permission".to_string()
        } else {
            format!("can't ask for administrator permission: {e}")
        }
    })?;
    let mut code = 0;
    // SAFETY: SEE_MASK_NOCLOSEPROCESS hands us the process handle, closed once here.
    let waited = unsafe {
        let waited = if WaitForSingleObject(info.hProcess, INFINITE) == WAIT_OBJECT_0 { GetExitCodeProcess(info.hProcess, &mut code) } else { Err(windows::core::Error::from_thread()) };
        let _ = CloseHandle(info.hProcess);
        waited
    };
    waited.map(|_| code).map_err(|e| format!("lost track of the installer: {e}"))
}

/// Removes what an update left behind: an install that didn't finish, and the replaced .exe.
pub fn cleanup() {
    let Ok(target) = install_target() else { return };
    #[cfg(windows)]
    for extension in [".new", ".old"] {
        let _ = std::fs::remove_file(beside(&target, extension));
    }
    if let Some(dir) = target.parent().and_then(|d| std::fs::read_dir(d).ok()) {
        for entry in dir.flatten() {
            if entry.file_name().to_string_lossy().starts_with(".winfer-update-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// Starts the installed update. It waits for this copy to quit (RELAUNCH_ENV) before taking over.
pub fn relaunch(exe: &Path) -> Result<(), String> {
    use std::process::Stdio;
    let mut cmd = std::process::Command::new(exe);
    cmd.env(RELAUNCH_ENV, "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    // Its own process group, so closing the terminal this copy was started from doesn't take it along.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn().map(drop).map_err(|e| format!("can't start the new version: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    const BODY: &str = "### Changes\n- Plugins: add Telegram, answer agents from your phone\n- Settings: start agents in auto-approve mode\n\n---\n### Install\nUnsigned builds.\n\n- not a change\n";

    fn rel(tag: &str, body: &str) -> Release {
        Release { tag_name: format!("{TAG_PREFIX}{tag}"), body: Some(body.into()), draft: false, prerelease: tag.contains('-') }
    }

    #[test]
    fn reads_the_changes_list_and_stops_at_the_footer() {
        assert_eq!(parse_changes(Some(BODY)), ["Plugins: add Telegram, answer agents from your phone", "Settings: start agents in auto-approve mode"]);
        assert_eq!(parse_changes(Some("- one\n* two\n---\n- three")), ["one", "two"]);
        assert!(parse_changes(None).is_empty() && parse_changes(Some("")).is_empty());
    }

    #[test]
    fn splits_the_area() {
        assert_eq!(split_area("Browser and side terminal: keep tabs"), (Some("Browser and side terminal".into()), "keep tabs".into()));
        assert_eq!(split_area("fix crash on start"), (None, "fix crash on start".into()));
        assert_eq!(split_area("see: this"), (None, "see: this".into()));
    }

    #[test]
    fn compares_versions() {
        assert_eq!(compare_versions("0.1.9", "0.1.10"), Ordering::Less);
        assert_eq!(compare_versions("v0.2.0", "0.1.43"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.43", "v0.1.43"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.102-7", "0.1.102"), Ordering::Less);
        assert_eq!(compare_versions("0.1.102-7", "0.1.101"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.102-9", "0.1.102-10"), Ordering::Less);
    }

    #[test]
    fn lists_what_changed_between_versions() {
        let releases = vec![rel("0.1.44", "- four"), rel("0.1.43", "- three"), rel("0.1.42", "- two"), rel("0.1.41", "- one"), Release { draft: true, ..rel("0.1.45", "- draft") }, Release { tag_name: "nebula-channel".into(), ..rel("0", "- feed") }];
        let got = releases_between(&releases, Some("0.1.41"), "0.1.43");
        assert_eq!(got, [ReleaseChanges { version: "0.1.43".into(), changes: vec!["three".into()] }, ReleaseChanges { version: "0.1.42".into(), changes: vec!["two".into()] }]);
        assert_eq!(releases_between(&releases, None, "0.1.44"), [ReleaseChanges { version: "0.1.44".into(), changes: vec!["four".into()] }]);
        let mut with_nebula = releases.clone();
        with_nebula.push(rel("0.1.45-3", "- u3"));
        with_nebula.push(rel("0.1.45-2", "- u2"));
        assert_eq!(releases_between(&with_nebula, Some("0.1.44"), "0.1.45-3").iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["0.1.45-3", "0.1.45-2"]);
        assert_eq!(releases_between(&with_nebula, Some("0.1.43"), "0.1.44").len(), 1);
    }

    #[test]
    fn reads_versions_from_release_tags() {
        let version = |tag: &str| Release { tag_name: tag.into(), ..rel("", "") }.version();
        assert_eq!(version("v0.1.107").as_deref(), Some("0.1.107"));
        assert_eq!(version("v0.1.107-132").as_deref(), Some("0.1.107-132"));
        for other in ["nebula-channel", "native-v0.1.0", "v0.1", "v0.1.2-rc1", "v0.1.2-"] {
            assert_eq!(version(other), None, "{other}");
        }
    }

    fn feed(version: &str, platforms: &[&str]) -> Feed {
        let bundle = |p: &&str| (p.to_string(), Bundle { url: format!("https://example/{p}"), signature: "sig".into() });
        Feed { version: version.into(), platforms: platforms.iter().map(bundle).collect() }
    }

    #[test]
    fn offers_only_a_newer_build_for_this_platform() {
        let here = platform();
        assert_eq!(offered(&feed("0.1.44", &[&here]), "0.1.43").map(|b| b.url), Some(format!("https://example/{here}")));
        assert!(offered(&feed("0.1.43", &[&here]), "0.1.43").is_none());
        // Never a downgrade: a Nebula build on the stable channel waits for its stable release.
        assert!(offered(&feed("0.1.43", &[&here]), "0.1.44-7").is_none());
        assert!(offered(&feed("0.1.44", &[&here]), "0.1.44-7").is_some());
        assert!(offered(&feed("0.1.44", &["plan9-mips"]), "0.1.43").is_none());
        assert_eq!(Channel::of_version("0.1.102-7"), Channel::Nebula);
    }

    #[test]
    fn reads_the_feed_release_yml_writes() {
        let json = r#"{"version":"0.1.108-3","notes":"- x","pub_date":"2026-11-21T00:00:00Z","platforms":{"darwin-aarch64":{"signature":"c2ln","url":"https://github.com/RickyDane/winfer/releases/download/v0.1.108-3/winfer-native_0.1.108-3_macos-aarch64.app.tar.gz"}}}"#;
        let f: Feed = serde_json::from_str(json).unwrap();
        assert_eq!(f.version, "0.1.108-3");
        assert_eq!(f.platforms["darwin-aarch64"].signature, "c2ln");
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(platform(), "darwin-aarch64");
        }
        assert!(Channel::Stable.feed().ends_with("/releases/latest/download/latest-native.json"));
        assert!(Channel::Nebula.feed().ends_with("/releases/download/nebula-channel/latest-native.json"));
    }

    #[test]
    fn checks_signatures_like_the_tauri_updater() {
        // `tauri signer sign` writes base64 of minisign's signature file; the key is base64 of its public key file.
        let b64 = |s: String| base64::engine::general_purpose::STANDARD.encode(s);
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pubkey = b64(pair.pk.to_box().unwrap().to_string());
        let data = b"winfer.app.tar.gz".to_vec();
        let signature = b64(minisign::sign(Some(&pair.pk), &pair.sk, std::io::Cursor::new(&data), None, None).unwrap().to_string());
        assert_eq!(verify(&data, &signature, &pubkey), Ok(()));
        assert!(verify(b"tampered", &signature, &pubkey).is_err());
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(verify(&data, &signature, &b64(other.pk.to_box().unwrap().to_string())).is_err());
        assert!(verify(&data, "not base64", &pubkey).is_err());
        // The real key, from tauri.conf.json, decodes.
        assert_ne!(verify(&data, &signature, &updater_pubkey()), Err("the update key is invalid".to_string()));
    }

    /// Unpacks a bundle over an installed copy the way an update does, in a scratch folder.
    #[cfg(not(windows))]
    #[test]
    fn swaps_the_update_in_for_the_installed_copy() {
        let root = std::env::temp_dir().join(format!("winfer-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (installed, build) = (root.join("Applications"), root.join("build"));
        // macOS replaces the whole .app; Linux the binary.
        let (target, packed, inner) = if cfg!(target_os = "macos") { (installed.join("winfer.app"), build.join("winfer.app"), "winfer.app/Contents/MacOS/winfer") } else { (installed.join("winfer"), build.join("winfer"), "winfer") };
        for (base, text) in [(&installed, "old"), (&build, "new")] {
            let file = base.join(inner);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
        }
        let archive = root.join("update.tar.gz");
        let ok = std::process::Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(&build).arg(packed.file_name().unwrap()).status().unwrap();
        assert!(ok.success());
        let staging = staging_dir(&target).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        let exe = swap_in(&target, &staging, &std::fs::read(&archive).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(installed.join(inner)).unwrap(), "new");
        assert_eq!(exe, installed.join(inner));
        // A broken download leaves the installed copy alone.
        assert!(swap_in(&target, &staging, b"not a tar.gz").is_err());
        assert_eq!(std::fs::read_to_string(installed.join(inner)).unwrap(), "new");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
