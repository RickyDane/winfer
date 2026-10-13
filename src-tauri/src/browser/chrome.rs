//! Finding, installing and launching the browser that backs the embedded panel.
//!
//! Order: `WINFER_CHROME` → managed Chrome for Testing headless shell (app data) → an installed
//! Chrome/Edge/Chromium/Brave run with `--headless=new`. If none exists the UI offers to download
//! the headless shell (~100 MB) from Google's Chrome for Testing feed.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Executable {
    pub path: PathBuf,
    /// "env", "managed" or "system".
    pub source: &'static str,
}

impl Executable {
    fn is_headless_shell(&self) -> bool {
        self.source == "managed"
    }
}

const PLATFORM: &str = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "mac-arm64"
} else if cfg!(target_os = "macos") {
    "mac-x64"
} else if cfg!(target_os = "windows") {
    "win64"
} else {
    "linux64"
};

const SHELL_BIN: &str = if cfg!(windows) { "chrome-headless-shell.exe" } else { "chrome-headless-shell" };

fn managed_exe(dir: &Path) -> PathBuf {
    dir.join(format!("chrome-headless-shell-{PLATFORM}")).join(SHELL_BIN)
}

fn system_candidates() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
        .map(PathBuf::from)
        .to_vec()
    } else if cfg!(windows) {
        let mut out = Vec::new();
        for var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                out.push(Path::new(&base).join(r"Google\Chrome\Application\chrome.exe"));
                out.push(Path::new(&base).join(r"Microsoft\Edge\Application\msedge.exe"));
            }
        }
        out
    } else {
        Vec::new()
    }
}

pub fn find(managed_dir: &Path) -> Option<Executable> {
    if let Some(path) = std::env::var_os("WINFER_CHROME").map(PathBuf::from).filter(|p| p.exists()) {
        return Some(Executable { path, source: "env" });
    }
    let managed = managed_exe(managed_dir);
    if managed.exists() {
        return Some(Executable { path: managed, source: "managed" });
    }
    system_candidates()
        .into_iter()
        .find(|p| p.exists())
        .map(|path| Executable { path, source: "system" })
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Downloads the Chrome for Testing headless shell into `managed_dir`. Uses curl and tar,
/// which ship with macOS and Windows 10+.
pub fn install(managed_dir: &Path) -> Result<Executable, String> {
    const FEED: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
    let feed: serde_json::Value = serde_json::from_str(&run(Command::new("curl").args(["-fsSL", FEED]))?)
        .map_err(|e| format!("bad Chrome for Testing feed: {e}"))?;
    let url = feed["channels"]["Stable"]["downloads"]["chrome-headless-shell"]
        .as_array()
        .and_then(|list| list.iter().find(|d| d["platform"] == PLATFORM))
        .and_then(|d| d["url"].as_str())
        .ok_or_else(|| format!("no headless shell download for {PLATFORM}"))?
        .to_string();

    std::fs::create_dir_all(managed_dir).map_err(|e| e.to_string())?;
    let zip = managed_dir.join("download.zip");
    run(Command::new("curl").args(["-fsSL", "-o"]).arg(&zip).arg(&url))?;
    let extracted = run(Command::new("tar").arg("-xf").arg(&zip).arg("-C").arg(managed_dir));
    let _ = std::fs::remove_file(&zip);
    extracted?;

    let exe = managed_exe(managed_dir);
    if !exe.exists() {
        return Err(format!("download finished but {} is missing", exe.display()));
    }
    Ok(Executable { path: exe, source: "managed" })
}

pub struct Launched {
    pub child: Child,
    pub port: u16,
    pub ws_url: String,
}

/// Starts the browser with a random debugging port bound to 127.0.0.1.
pub fn launch(exe: &Executable, profile_dir: &Path) -> Result<Launched, String> {
    std::fs::create_dir_all(profile_dir).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&exe.path);
    cmd.arg("--remote-debugging-port=0")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .args(["--no-first-run", "--no-default-browser-check", "--disable-features=Translate,MediaRouter", "--mute-audio"]);
    if !exe.is_headless_shell() {
        cmd.arg("--headless=new");
    }
    cmd.arg("about:blank").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("couldn't start {}: {e}", exe.path.display()))?;
    let stderr = child.stderr.take().expect("piped stderr");

    // Chrome prints "DevTools listening on ws://127.0.0.1:PORT/devtools/browser/<id>" on stderr.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !sent {
                if let Some(url) = line.split("DevTools listening on ").nth(1) {
                    let _ = tx.send(url.trim().to_string());
                    sent = true;
                }
            }
            // Keep draining so Chrome never blocks on a full pipe.
        }
    });

    let ws_url = match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(url) => url,
        Err(_) => {
            let _ = child.kill();
            return Err("the browser didn't report a DevTools endpoint".into());
        }
    };
    let port = ws_url
        .split("127.0.0.1:")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| format!("unexpected DevTools URL: {ws_url}"))?;
    Ok(Launched { child, port, ws_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a local Chrome (or WINFER_CHROME); skipped otherwise.
    #[test]
    fn launches_and_reports_endpoint() {
        let tmp = std::env::temp_dir().join(format!("winfer-chrome-{:x}", rand::random::<u32>()));
        let Some(exe) = find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let mut launched = launch(&exe, &tmp.join("profile")).unwrap();
        assert!(launched.ws_url.starts_with("ws://127.0.0.1:"));
        assert!(launched.port > 0);
        let _ = launched.child.kill();
        let _ = launched.child.wait();
    }
}
