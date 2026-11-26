//! Updates: the newest release's winferd for this platform, checked against its sha256, put in place
//! of the running binary, then the process restarts into it (same pid, so systemd, launchd or a
//! terminal keep it). Docker images update by pulling a new image instead.

use sha2::Digest;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use winfer_core::version::compare_versions;

const RELEASES: &str = "https://github.com/RickyDane/winfer/releases";
const IMAGE: &str = "ghcr.io/rickydane/winferd";
const MAX_DOWNLOAD: u64 = 300 << 20;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The newest release, as last checked.
static LATEST: Mutex<Option<String>> = Mutex::new(None);

/// Where releases are (`WINFERD_UPDATE_BASE` for tests: a folder layout like GitHub's).
fn base() -> String {
    std::env::var("WINFERD_UPDATE_BASE").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| RELEASES.into()).trim_end_matches('/').to_string()
}

/// This platform's release asset (without `.tar.gz`).
pub fn asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("winferd-linux-x86_64"),
        ("linux", "aarch64") => Some("winferd-linux-aarch64"),
        ("macos", "x86_64") => Some("winferd-macos-x86_64"),
        ("macos", "aarch64") => Some("winferd-macos-aarch64"),
        _ => None,
    }
}

pub fn in_docker() -> bool {
    Path::new("/.dockerenv").exists() || std::env::var_os("WINFERD_DOCKER").is_some()
}

/// How to update a container (it can't replace its own image).
pub fn docker_hint() -> String {
    format!("Pull the new image and recreate the container: docker pull {IMAGE} && docker rm -f winferd && docker run … (same volume)")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).build().into()
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut res = agent().get(url).call().map_err(|e| format!("{url}: {e}"))?;
    res.body_mut().with_config().limit(MAX_DOWNLOAD).read_to_vec().map_err(|e| format!("{url}: {e}"))
}

/// `check`'s error when the newest release has no winferd in it (none has yet, or test builds
/// only come as pre-releases).
pub const NO_RELEASE: &str = "The newest winfer release has no winferd yet. Test builds: winferd update --version <tag>";

/// The newest release's tag (`winferd-version.txt`, uploaded with every release).
pub fn check() -> Result<String, String> {
    let text = get(&format!("{}/latest/download/winferd-version.txt", base())).map_err(|e| if e.contains("404") { NO_RELEASE.to_string() } else { e })?;
    let text = String::from_utf8(text).map_err(|e| e.to_string())?;
    let tag = text.trim().to_string();
    if !tag.starts_with('v') || tag.len() > 40 {
        return Err(format!("unexpected version {tag:?}"));
    }
    *LATEST.lock().unwrap() = Some(tag.clone());
    Ok(tag)
}

/// A newer release than this one, if the last check found one.
pub fn available() -> Option<String> {
    LATEST.lock().unwrap().clone().filter(|tag| compare_versions(tag, VERSION) == std::cmp::Ordering::Greater)
}

pub fn latest_known() -> Option<String> {
    LATEST.lock().unwrap().clone()
}

/// Checks now and then every 6 hours.
pub fn watch() {
    std::thread::spawn(|| {
        loop {
            // (Until a release carries winferd: nothing to say.)
            if let Err(e) = check()
                && e != NO_RELEASE
            {
                eprintln!("update check: {e}");
            }
            std::thread::sleep(Duration::from_secs(6 * 3600));
        }
    });
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `tag` and puts it in place of `exe`. Returns the version installed.
pub fn install(tag: &str, exe: &Path) -> Result<String, String> {
    if in_docker() {
        return Err(docker_hint());
    }
    let asset = asset().ok_or("no winferd release for this platform")?;
    let url = format!("{}/download/{tag}/{asset}.tar.gz", base());
    let archive = get(&url)?;
    let want = String::from_utf8_lossy(&get(&format!("{url}.sha256"))?).split_whitespace().next().unwrap_or_default().to_lowercase();
    if hex(&sha2::Sha256::digest(&archive)) != want {
        return Err("checksum mismatch: not updating".into());
    }
    let dir = exe.parent().ok_or("no folder for the binary")?;
    let work = dir.join(format!(".winferd-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let result = (|| {
        let tgz = work.join("winferd.tar.gz");
        std::fs::write(&tgz, &archive).map_err(|e| e.to_string())?;
        let out = std::process::Command::new("tar").arg("-xzf").arg(&tgz).arg("-C").arg(&work).arg("winferd").output().map_err(|e| format!("tar: {e}"))?;
        if !out.status.success() {
            return Err(format!("tar: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        let new = work.join("winferd");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        // The new binary must run here before it replaces this one.
        let version = std::process::Command::new(&new).arg("--version").output().map_err(|e| format!("the new winferd doesn't run: {e}"))?;
        let version = String::from_utf8_lossy(&version.stdout).trim().trim_start_matches("winferd ").to_string();
        if version.is_empty() {
            return Err("the new winferd doesn't run here".into());
        }
        std::fs::rename(&new, exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        Ok(version)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The running binary's path, before an update replaces it (Linux names a replaced one "… (deleted)").
pub fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string()).map(|p| {
        let s = p.display().to_string();
        PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s))
    })
}

/// Becomes the (new) binary at `exe` with the same arguments. Files are close-on-exec, so the
/// instance lock and the agents' terminals go with the old image.
pub fn restart(exe: &Path) -> ! {
    // A clean exit for the diagnostics, and the browsers it started closed.
    winfer_core::backend::shutdown();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
        eprintln!("restart failed: {err}");
    }
    std::process::exit(75)
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_platform_has_an_asset() {
        if cfg!(any(target_os = "linux", target_os = "macos")) && cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
            assert!(super::asset().is_some());
        }
    }
}
