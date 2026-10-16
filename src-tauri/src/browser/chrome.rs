//! Finding, installing and launching the browser that backs the embedded panel.
//!
//! Order: `WINFER_CHROME` → managed Chrome for Testing headless shell (app data) → an installed
//! Chrome/Edge/Chromium/Brave run with `--headless=new`. If none exists the UI offers to download
//! the headless shell (~100 MB) from Google's Chrome for Testing feed.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

const SHELL_BIN: &str = if cfg!(windows) {
    "chrome-headless-shell.exe"
} else {
    "chrome-headless-shell"
};

fn managed_exe(dir: &Path) -> PathBuf {
    dir.join(format!("chrome-headless-shell-{PLATFORM}"))
        .join(SHELL_BIN)
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
    if let Some(path) = std::env::var_os("WINFER_CHROME")
        .map(PathBuf::from)
        .filter(|p| p.exists())
    {
        return Some(Executable {
            path,
            source: "env",
        });
    }
    let managed = managed_exe(managed_dir);
    if managed.exists() {
        return Some(Executable {
            path: managed,
            source: "managed",
        });
    }
    system_candidates()
        .into_iter()
        .find(|p| p.exists())
        .map(|path| Executable {
            path,
            source: "system",
        })
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = crate::env::no_window(cmd).output().map_err(|e| e.to_string())?;
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
    let feed: serde_json::Value =
        serde_json::from_str(&run(Command::new("curl").args(["-fsSL", FEED]))?)
            .map_err(|e| format!("bad Chrome for Testing feed: {e}"))?;
    let url = feed["channels"]["Stable"]["downloads"]["chrome-headless-shell"]
        .as_array()
        .and_then(|list| list.iter().find(|d| d["platform"] == PLATFORM))
        .and_then(|d| d["url"].as_str())
        .ok_or_else(|| format!("no headless shell download for {PLATFORM}"))?
        .to_string();

    std::fs::create_dir_all(managed_dir).map_err(|e| e.to_string())?;
    let zip = managed_dir.join("download.zip");
    run(Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&zip)
        .arg(&url))?;
    let extracted = run(Command::new("tar")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(managed_dir));
    let _ = std::fs::remove_file(&zip);
    extracted?;

    let exe = managed_exe(managed_dir);
    if !exe.exists() {
        return Err(format!(
            "download finished but {} is missing",
            exe.display()
        ));
    }
    Ok(Executable {
        path: exe,
        source: "managed",
    })
}

pub struct Launched {
    pub child: Child,
    pub port: u16,
}

/// A free port on 127.0.0.1 for the browser's DevTools endpoint.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("no free port for the browser: {e}"))
}

/// Holds the DevTools port of the browser last started on a profile (Chrome only writes its own
/// `DevToolsActivePort` when it picks the port).
const PORT_FILE: &str = "winfer-devtools-port";

/// Starts the browser with its DevTools endpoint on a port chosen up front. Returns as soon as
/// the process runs: callers (terminal spawns on the UI thread) must never wait for the
/// browser. Use `ws_url` to get the endpoint once it's up.
pub fn launch(exe: &Executable, profile_dir: &Path) -> Result<Launched, String> {
    std::fs::create_dir_all(profile_dir).map_err(|e| e.to_string())?;
    close_running(profile_dir);
    let port = free_port()?;
    let mut cmd = Command::new(&exe.path);
    cmd.arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-features=Translate,MediaRouter",
            "--mute-audio",
        ]);
    if !exe.is_headless_shell() {
        cmd.arg("--headless=new");
    }
    cmd.arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::env::no_window(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", exe.path.display()))?;
    #[cfg(windows)]
    kill_with_app(&child);
    let _ = std::fs::write(profile_dir.join(PORT_FILE), port.to_string());
    Ok(Launched { child, port })
}

/// Puts the browser in a job object that Windows tears down when winfer exits, crash included,
/// so no browser outlives the app. Replaces the PowerShell orphan sweep that endpoint
/// protection (Sophos "Lockdown") blocked on startup.
#[cfg(windows)]
fn kill_with_app(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    // The handle is deliberately never closed: the OS closes it when winfer exits, which kills
    // the job. Stored as usize because raw handles aren't Sync.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        );
        job as usize
    });
    if job != 0 {
        unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
    }
}

/// Quits a browser left running on this profile by an earlier run (crash, force quit, or a
/// version without the Windows job object). It holds the profile lock, so a new browser would
/// hand its launch over to it and exit at once. Found through the `DevToolsActivePort` file
/// recorded in the profile at launch and told to close over CDP: no process sweep, which endpoint
/// protection blocks on Windows. Returns quickly when nothing is running.
fn close_running(profile_dir: &Path) {
    use std::net::{SocketAddr, TcpStream};
    use tokio_tungstenite::tungstenite::{self, Message};

    let Some(port) = std::fs::read_to_string(profile_dir.join(PORT_FILE))
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
    else {
        return;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let answers = || TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok();
    if !answers() {
        return;
    }
    // Anything else on a reused port has no DevTools endpoint and is left alone.
    let Some(url) = ws_url(port) else {
        return;
    };
    let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let Ok((mut ws, _)) = tungstenite::client(url, stream) else {
        return;
    };
    let _ = ws.send(Message::text(r#"{"id":1,"method":"Browser.close"}"#));
    let _ = ws.read();
    // The port closes as the browser shuts down; give it a moment to release the profile.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while answers() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(300));
}

/// One attempt at reading the browser-level WebSocket URL from `/json/version`.
pub fn ws_url(port: u16) -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into();
    let mut res = agent
        .get(&format!("http://127.0.0.1:{port}/json/version"))
        .call()
        .ok()?;
    let json: serde_json::Value = res.body_mut().read_json().ok()?;
    json["webSocketDebuggerUrl"].as_str().map(String::from)
}

/// Waits (blocking) until the endpoint answers.
#[cfg(test)]
pub fn wait_ws_url(port: u16, timeout: Duration) -> Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(url) = ws_url(port) {
            return Ok(url);
        }
        if std::time::Instant::now() > deadline {
            return Err("the browser didn't open its DevTools endpoint".into());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Kills a test's browser even when an assertion panics, so failed runs don't leak Chrome.
#[cfg(test)]
pub(crate) struct ChromeGuard(pub Child);

#[cfg(test)]
impl Drop for ChromeGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
        let launched = launch(&exe, &tmp.join("profile")).unwrap();
        let _guard = ChromeGuard(launched.child);
        let url = wait_ws_url(launched.port, Duration::from_secs(45)).unwrap();
        assert!(url.starts_with(&format!("ws://127.0.0.1:{}/", launched.port)), "{url}");
    }

    /// A browser left running on the profile would take over the next launch; it gets closed.
    #[test]
    fn closes_a_leftover_browser_on_the_profile() {
        let tmp = std::env::temp_dir().join(format!("winfer-chrome-{:x}", rand::random::<u32>()));
        let Some(exe) = find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let profile = tmp.join("profile");
        let first = launch(&exe, &profile).unwrap();
        let mut leftover = ChromeGuard(first.child);
        wait_ws_url(first.port, Duration::from_secs(45)).unwrap();

        let second = launch(&exe, &profile).unwrap();
        let _guard = ChromeGuard(second.child);
        assert!(
            leftover.0.try_wait().unwrap().is_some(),
            "the leftover browser still runs"
        );
        wait_ws_url(second.port, Duration::from_secs(45)).unwrap();
    }
}
