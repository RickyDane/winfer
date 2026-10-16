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

/// Chrome's exit code when another browser holds the profile (`RESULT_CODE_PROFILE_IN_USE`).
pub const PROFILE_IN_USE: i32 = 21;

/// Starts the browser with its DevTools endpoint on a port chosen up front. Returns as soon as
/// the process runs: callers (terminal spawns on the UI thread) must never wait for the
/// browser. Use `ws_url` to get the endpoint once it's up.
pub fn launch(exe: &Executable, profile_dir: &Path) -> Result<Launched, String> {
    let port = free_port()?;
    let child = spawn(exe, profile_dir, port)?;
    Ok(Launched { child, port })
}

/// Starts the browser on `port`, first freeing the profile from any browser still on it. Also
/// used to retry on the same port, so the CDP URL agents already got stays valid.
pub fn spawn(exe: &Executable, profile_dir: &Path, port: u16) -> Result<Child, String> {
    std::fs::create_dir_all(profile_dir).map_err(|e| e.to_string())?;
    reclaim(profile_dir);
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
    // Own process group, so `terminate` can take the helpers down with the browser.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    crate::env::no_window(&mut cmd);
    #[cfg(windows)]
    let child = job::spawn(&mut cmd);
    #[cfg(not(windows))]
    let child = cmd.spawn().map_err(|e| e.to_string());
    let child = child.map_err(|e| format!("couldn't start {}: {e}", exe.path.display()))?;
    let _ = std::fs::write(profile_dir.join(PORT_FILE), port.to_string());
    Ok(child)
}

/// Asks the browser on `port` to quit (so it flushes cookies and the profile), then force-kills
/// it and its helpers. Blocks for a few seconds at most.
pub fn terminate(child: &mut Child, port: u16) {
    if child.try_wait().ok().flatten().is_none() {
        #[cfg(unix)]
        let _ = port;
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        #[cfg(windows)]
        close_over_cdp(port);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while child.try_wait().ok().flatten().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    // Helpers that outlived the browser.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    job::kill_tree(child.id());
}

/// Frees a profile for a new browser: ends whatever browser still runs on it (an earlier run's
/// orphan, or one still shutting down), which would otherwise take the launch over and make
/// the new browser exit with `PROFILE_IN_USE`, and clears the lock it left behind.
fn reclaim(profile_dir: &Path) {
    close_running(profile_dir);
    #[cfg(unix)]
    {
        holders::end(|dir| dir == profile_dir);
        holders::clear_lock(profile_dir);
    }
    #[cfg(windows)]
    job::end_holders(profile_dir);
}

/// Ends browsers left on any profile in `profiles_dir` by a run that didn't exit cleanly and
/// clears their locks. Waits until they're gone, so launches right after don't race them.
pub fn reclaim_all(profiles_dir: &Path) {
    #[cfg(unix)]
    holders::end(|dir| dir.parent() == Some(profiles_dir));
    if let Ok(entries) = std::fs::read_dir(profiles_dir) {
        for entry in entries.flatten() {
            #[cfg(unix)]
            holders::clear_lock(&entry.path());
            #[cfg(windows)]
            job::end_holders(&entry.path());
        }
    }
}

/// Browser processes found by their `--user-data-dir`, so ones from earlier runs (which winfer
/// no longer holds a handle to) are found too.
#[cfg(unix)]
mod holders {
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    const FLAG: &str = "--user-data-dir=";

    /// The profile a command line runs on. The path may contain spaces; the next `--` flag
    /// (or our trailing `about:blank`) ends it.
    fn profile_of(command: &str) -> Option<&Path> {
        let rest = &command[command.find(FLAG)? + FLAG.len()..];
        let value = rest.find(" --").map_or(rest, |i| &rest[..i]);
        Some(Path::new(value.strip_suffix(" about:blank").unwrap_or(value)))
    }

    fn pids(on: &impl Fn(&Path) -> bool) -> Vec<i32> {
        let Ok(out) = Command::new("ps").args(["-axww", "-o", "pid=,command="]).output() else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (pid, command) = line.trim_start().split_once(' ')?;
                profile_of(command)
                    .filter(|dir| on(dir))
                    .and_then(|_| pid.parse().ok())
            })
            .filter(|&pid| pid != std::process::id() as i32)
            .collect()
    }

    fn signal_until_gone(on: &impl Fn(&Path) -> bool, signal: i32, wait: Duration) -> bool {
        for pid in pids(on) {
            unsafe { libc::kill(pid, signal) };
        }
        let deadline = Instant::now() + wait;
        loop {
            if pids(on).is_empty() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// SIGTERM, then SIGKILL for browsers that hang. Returns at once when none run.
    pub fn end(on: impl Fn(&Path) -> bool) {
        if pids(&on).is_empty() {
            return;
        }
        if !signal_until_gone(&on, libc::SIGTERM, Duration::from_secs(3)) {
            signal_until_gone(&on, libc::SIGKILL, Duration::from_secs(2));
        }
    }

    /// Removes the singleton lock of a profile no browser runs on. Chrome treats a lock naming a
    /// live process as in use, and after a crash that pid may belong to anything by now.
    pub fn clear_lock(profile_dir: &Path) {
        if !pids(&|dir: &Path| dir == profile_dir).is_empty() {
            return;
        }
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            let _ = std::fs::remove_file(profile_dir.join(name));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_the_profile_from_a_command_line() {
            let cmd = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=1 --user-data-dir=/Users/a/Library/Application Support/x/browser-profiles/ws-1 --no-first-run about:blank";
            assert_eq!(
                profile_of(cmd),
                Some(Path::new("/Users/a/Library/Application Support/x/browser-profiles/ws-1"))
            );
            assert_eq!(
                profile_of("chrome --user-data-dir=/p/ws-1 about:blank"),
                Some(Path::new("/p/ws-1"))
            );
            assert_eq!(profile_of("chrome --user-data-dir=/p/ws-12"), Some(Path::new("/p/ws-12")));
            assert_eq!(profile_of("chrome --type=renderer"), None);
        }
    }
}

/// Windows: every browser runs in its own job object that kills its whole process tree when
/// closed, which Windows does when winfer exits, crash included. Orphans of runs where that
/// failed are found the way Chrome itself finds the profile's owner, not by a process sweep
/// (PowerShell/WMI), which endpoint protection (Sophos "Lockdown") blocks.
#[cfg(windows)]
mod job {
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::sync::{LazyLock, Mutex};

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::RestartManager::{
        CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
        RmStartSession,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, ResumeThread,
        THREAD_SUSPEND_RESUME, TerminateProcess, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowThreadProcessId, HWND_MESSAGE,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Browser pid → its job. Stored as usize because raw handles aren't Send.
    static JOBS: LazyLock<Mutex<HashMap<u32, usize>>> = LazyLock::new(Mutex::default);

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain([0]).collect()
    }

    fn new_job() -> Option<HANDLE> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            );
            Some(job)
        }
    }

    /// Starts the browser suspended and only lets it run once it's in its job, so no helper it
    /// starts early (crashpad, GPU, network) escapes the job and outlives winfer.
    pub fn spawn(cmd: &mut Command) -> Result<Child, String> {
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        if let Some(job) = new_job() {
            if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } != 0 {
                JOBS.lock().unwrap().insert(child.id(), job as usize);
            } else {
                unsafe { CloseHandle(job) };
            }
        }
        if !resume(child.id()) {
            kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Err("couldn't resume the browser process".into());
        }
        Ok(child)
    }

    fn resume(pid: u32) -> bool {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut resumed = false;
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        resumed |= ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            resumed
        }
    }

    /// Kills the browser's whole process tree, helpers included.
    pub fn kill_tree(pid: u32) {
        if let Some(job) = JOBS.lock().unwrap().remove(&pid) {
            unsafe {
                TerminateJobObject(job as HANDLE, 1);
                CloseHandle(job as HANDLE);
            }
        }
    }

    /// The browser owning the profile, found as Chrome finds it before handing a launch over:
    /// its message window, titled with the profile path.
    fn window_owner(profile_dir: &Path) -> Option<u32> {
        let class = wide(OsStr::new("Chrome_MessageWindow"));
        let title = wide(profile_dir.as_os_str());
        let mut pid = 0;
        unsafe {
            let hwnd = FindWindowExW(HWND_MESSAGE, std::ptr::null_mut(), class.as_ptr(), title.as_ptr());
            if hwnd.is_null() {
                return None;
            }
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        (pid != 0).then_some(pid)
    }

    /// Processes holding the profile's `lockfile` open, via the Restart Manager.
    fn lock_holders(lockfile: &Path) -> Vec<u32> {
        if !lockfile.exists() {
            return Vec::new();
        }
        let path = wide(lockfile.as_os_str());
        let mut out = Vec::new();
        unsafe {
            let mut session = 0;
            let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
            if RmStartSession(&mut session, 0, key.as_mut_ptr()) != 0 {
                return out;
            }
            let files = [path.as_ptr()];
            if RmRegisterResources(session, 1, files.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
                let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
                for _ in 0..3 {
                    let (mut needed, mut count, mut reasons) = (0, infos.len() as u32, 0);
                    let res = RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons);
                    if res == ERROR_MORE_DATA {
                        infos = vec![std::mem::zeroed(); needed as usize];
                        continue;
                    }
                    if res == 0 {
                        out = infos[..count as usize].iter().map(|i| i.Process.dwProcessId).collect();
                    }
                    break;
                }
            }
            RmEndSession(session);
        }
        out
    }

    /// Only browsers get killed: a virus scanner may hold the lockfile for a moment too.
    fn is_browser(process: HANDLE) -> bool {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) } == 0 {
            return false;
        }
        let image = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
        let name = image.rsplit('\\').next().unwrap_or_default().to_string();
        let custom = std::env::var_os("WINFER_CHROME").and_then(|p| {
            Path::new(&p).file_name().map(|n| n.to_string_lossy().to_lowercase())
        });
        ["chrome.exe", "msedge.exe", "brave.exe", "chromium.exe", super::SHELL_BIN]
            .contains(&name.as_str())
            || custom.as_deref() == Some(name.as_str())
    }

    /// Ends browsers still on the profile (orphans of an earlier run, or one still shutting
    /// down) and waits until they're gone. Returns at once when there are none.
    pub fn end_holders(profile_dir: &Path) {
        let mut pids = lock_holders(&profile_dir.join("lockfile"));
        pids.extend(window_owner(profile_dir));
        pids.sort_unstable();
        pids.dedup();
        for pid in pids {
            if pid == std::process::id() {
                continue;
            }
            unsafe {
                let access = PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
                let process = OpenProcess(access, 0, pid);
                if process.is_null() {
                    continue;
                }
                if is_browser(process) {
                    TerminateProcess(process, 1);
                    WaitForSingleObject(process, 5000);
                }
                CloseHandle(process);
            }
        }
        // Deletable only once nobody holds it.
        let _ = std::fs::remove_file(profile_dir.join("lockfile"));
    }
}

/// Quits a browser left running on this profile by an earlier run (crash, force quit, or a
/// version without the Windows job object). It holds the profile lock, so a new browser would
/// hand its launch over to it and exit at once. Found through the `DevToolsActivePort` file
/// recorded in the profile at launch and told to close over CDP: no process sweep, which endpoint
/// protection blocks on Windows. Returns quickly when nothing is running.
fn close_running(profile_dir: &Path) {
    let Some(port) = std::fs::read_to_string(profile_dir.join(PORT_FILE))
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
    else {
        return;
    };
    close_over_cdp(port);
}

/// Tells the browser on `port` to close over CDP and waits (up to ~3 s) until it has. Anything
/// else on a reused port has no DevTools endpoint and is left alone.
fn close_over_cdp(port: u16) {
    use std::net::{SocketAddr, TcpStream};
    use tokio_tungstenite::tungstenite::{self, Message};

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let answers = || TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok();
    if !answers() {
        return;
    }
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

    /// A hung leftover answers neither CDP nor SIGTERM; it still gets killed and its lock cleared.
    #[cfg(unix)]
    #[test]
    fn kills_a_hung_leftover_browser() {
        let tmp = std::env::temp_dir().join(format!("winfer-chrome-{:x}", rand::random::<u32>()));
        let Some(exe) = find(&tmp.join("managed")) else {
            eprintln!("no Chrome found; skipping");
            return;
        };
        let profile = tmp.join("profile");
        let first = launch(&exe, &profile).unwrap();
        let mut leftover = ChromeGuard(first.child);
        wait_ws_url(first.port, Duration::from_secs(45)).unwrap();
        unsafe { libc::kill(leftover.0.id() as i32, libc::SIGSTOP) };

        let second = launch(&exe, &profile).unwrap();
        let _guard = ChromeGuard(second.child);
        assert!(
            leftover.0.try_wait().unwrap().is_some(),
            "the hung browser still runs"
        );
        wait_ws_url(second.port, Duration::from_secs(45)).unwrap();
    }
}
