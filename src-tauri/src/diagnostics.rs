//! Persistent diagnostics: the file log, the crash log and the memory timeline.
//!
//! Why this exists: release builds on Windows have no console
//! (`windows_subsystem = "windows"`), so `eprintln!` and webview `console.*`
//! vanish, and an out-of-memory kill leaves no panic behind at all. Everything
//! a bug report needs therefore goes to files under `<app data>/logs/`:
//!
//! - `winfer.log` — structured app log (this module, plus frontend entries
//!   forwarded over IPC). Rotated at 5 MiB, three backups kept.
//! - `crash.log` — one block per Rust panic (and, on Windows, per native
//!   exception, with a `.dmp` next to it): message, location, backtrace,
//!   memory snapshot, recent breadcrumbs and the tail of the memory timeline.
//! - The memory sampler (below) appends an `rss=…` line every 30 s and
//!   whenever growth looks dangerous, so the run-up to an OOM kill — which
//!   produces neither a panic nor a dump — is still on disk afterwards.
//! - `last-session.json` — rewritten with every sample. If it is still there
//!   and unmarked at the next start, the previous run died without exiting
//!   (OOM kill, native crash, power loss), and that fact is logged.
//!
//! Overhead is deliberately tiny: one locked writer, no formatting on hot
//! paths unless a line is actually emitted, and the sampler sleeps 5 s
//! between reads. Nothing here may allocate per PTY byte or per token.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

// ---------------------------------------------------------------- levels

pub const ERROR: u8 = 0;
pub const WARN: u8 = 1;
pub const INFO: u8 = 2;
pub const DEBUG: u8 = 3;

fn level_name(level: u8) -> &'static str {
    match level {
        ERROR => "ERROR",
        WARN => "WARN",
        INFO => "INFO",
        _ => "DEBUG",
    }
}

fn parse_level(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "error" => ERROR,
        "warn" => WARN,
        "debug" | "trace" => DEBUG,
        _ => INFO,
    }
}

/// Current verbosity. `WINFER_LOG=debug` wins at startup; Settings can raise
/// it at runtime (persisted as `logs/verbose.flag`).
static LEVEL: AtomicU8 = AtomicU8::new(INFO);
static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

fn enabled(level: u8) -> bool {
    level <= LEVEL.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------- time

/// Milliseconds since the epoch, for log lines.
fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// `2026-11-03T14:22:31.123Z` for `ms` since the epoch (civil-date conversion,
/// no chrono dependency).
fn iso8601(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        tod / 3600,
        (tod / 60) % 60,
        tod % 60,
    )
}

// ---------------------------------------------------------------- writer

const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const LOG_BACKUPS: u32 = 3;
const CRASH_MAX_BYTES: u64 = 1024 * 1024;

struct Writer {
    dir: PathBuf,
    file: Option<std::fs::File>,
    size: u64,
}

impl Writer {
    fn open(dir: &Path) -> Writer {
        let path = dir.join("winfer.log");
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        Writer { dir: dir.to_path_buf(), file, size }
    }

    fn rotate(&mut self) {
        self.file = None;
        rotate_files(&self.dir.join("winfer.log"), LOG_BACKUPS);
        self.file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("winfer.log"))
            .ok();
        self.size = 0;
    }

    fn write_line(&mut self, line: &str) {
        if self.size > LOG_MAX_BYTES {
            self.rotate();
        }
        if let Some(file) = &mut self.file {
            // One write per line, then flush: a following OOM kill must not
            // take the last (most important) lines with it. File writes are
            // page-cache cheap; this is never on a per-byte path.
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
            self.size += line.len() as u64;
        }
    }
}

/// `winfer.log` → `winfer.log.1` → `.2` → `.3` (oldest dropped).
fn rotate_files(path: &Path, backups: u32) {
    let with_suffix = |n: u32| {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    };
    let _ = std::fs::remove_file(with_suffix(backups));
    for n in (1..backups).rev() {
        let _ = std::fs::rename(with_suffix(n), with_suffix(n + 1));
    }
    let _ = std::fs::rename(path, with_suffix(1));
}

// ---------------------------------------------------------------- state

const MAX_BREADCRUMBS: usize = 200;
/// 5 s samples for the last 24 minutes: enough to see a leak's slope.
const MAX_SAMPLES: usize = 288;

struct State_ {
    writer: Mutex<Option<Writer>>,
    breadcrumbs: Mutex<VecDeque<String>>,
    samples: Mutex<VecDeque<Sample>>,
    gauges: Mutex<HashMap<String, i64>>,
    started: Instant,
    dir: PathBuf,
    previous_unexpected_exit: bool,
    last_rss: AtomicU64,
    peak_rss: AtomicU64,
}

static STATE: OnceLock<State_> = OnceLock::new();

#[derive(Clone, Copy)]
struct Sample {
    at_ms: u128,
    rss: u64,
}

fn state() -> Option<&'static State_> {
    STATE.get()
}

/// A named counter/gauge other modules publish for the memory timeline
/// (e.g. live PTY sessions, cached scrollback bytes). Cheap: one short lock.
pub fn set_gauge(name: &str, value: i64) {
    if let Some(s) = state() {
        if let Ok(mut gauges) = s.gauges.lock() {
            gauges.insert(name.to_string(), value);
        }
    }
}

/// Last thing the user/app did, kept in a ring and dumped into crash reports.
/// Keep these short and free of secrets (prompts, file contents).
pub fn breadcrumb(message: impl AsRef<str>) {
    if let Some(s) = state() {
        if let Ok(mut crumbs) = s.breadcrumbs.lock() {
            if crumbs.len() >= MAX_BREADCRUMBS {
                crumbs.pop_front();
            }
            crumbs.push_back(format!("{} {}", iso8601(now_ms()), truncate(message.as_ref(), 300)));
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

// ---------------------------------------------------------------- memory

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub system_total_bytes: Option<u64>,
    pub system_available_bytes: Option<u64>,
}

/// This process's current (and peak) resident memory plus system headroom.
/// `None` fields mean "couldn't read it here", never zero.
pub fn memory_snapshot() -> MemorySnapshot {
    platform_memory()
}

#[cfg(target_os = "windows")]
fn platform_memory() -> MemorySnapshot {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut snap = MemorySnapshot::default();
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            counters.cb,
        ) != 0
        {
            snap.rss_bytes = Some(counters.WorkingSetSize as u64);
            snap.peak_rss_bytes = Some(counters.PeakWorkingSetSize as u64);
        }
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) != 0 {
            snap.system_total_bytes = Some(status.ullTotalPhys);
            snap.system_available_bytes = Some(status.ullAvailPhys);
        }
    }
    snap
}

#[cfg(target_os = "macos")]
fn platform_memory() -> MemorySnapshot {
    // mach task_info(MACH_TASK_BASIC_INFO): current + peak resident size.
    #[repr(C)]
    #[derive(Default)]
    struct TimeValue {
        seconds: i32,
        microseconds: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: TimeValue,
        system_time: TimeValue,
        policy: i32,
        suspend_count: i32,
    }
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: u32, info: *mut MachTaskBasicInfo, count: *mut u32) -> i32;
    }
    const MACH_TASK_BASIC_INFO: u32 = 20;

    let mut snap = MemorySnapshot::default();
    unsafe {
        let mut info = MachTaskBasicInfo::default();
        let mut count = (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;
        if task_info(mach_task_self(), MACH_TASK_BASIC_INFO, &mut info, &mut count) == 0 {
            snap.rss_bytes = Some(info.resident_size);
            snap.peak_rss_bytes = Some(info.resident_size_max);
        }
        let page = libc::sysconf(libc::_SC_PAGESIZE);
        if page > 0 {
            let phys = libc::sysconf(libc::_SC_PHYS_PAGES);
            if phys > 0 {
                snap.system_total_bytes = Some((phys as u64) * (page as u64));
            }
            // host_statistics64(HOST_VM_INFO64): first field is free pages.
            #[repr(C)]
            #[derive(Default)]
            struct VmStatistics64 {
                free_count: u32,
                _rest: [u32; 24],
            }
            unsafe extern "C" {
                fn mach_host_self() -> u32;
                fn host_statistics64(host: u32, flavor: i32, info: *mut VmStatistics64, count: *mut u32) -> i32;
            }
            const HOST_VM_INFO64: i32 = 4;
            let mut vm = VmStatistics64::default();
            let mut count = (std::mem::size_of::<VmStatistics64>() / std::mem::size_of::<u32>()) as u32;
            if host_statistics64(mach_host_self(), HOST_VM_INFO64, &mut vm, &mut count) == 0 {
                snap.system_available_bytes = Some((vm.free_count as u64) * (page as u64));
            }
        }
    }
    snap
}

#[cfg(target_os = "linux")]
fn platform_memory() -> MemorySnapshot {
    let mut snap = MemorySnapshot::default();
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page > 0 {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            snap.rss_bytes = parse_statm_rss(&statm, page as u64);
        }
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmHWM:") {
                    snap.peak_rss_bytes = parse_kb_value(rest);
                }
            }
        }
    }
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                snap.system_total_bytes = parse_kb_value(rest);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                snap.system_available_bytes = parse_kb_value(rest);
            }
        }
    }
    snap
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn platform_memory() -> MemorySnapshot {
    MemorySnapshot::default()
}

/// Resident pages (2nd field of /proc/self/statm) → bytes. Pure, for tests.
#[cfg(any(target_os = "linux", test))]
fn parse_statm_rss(statm: &str, page_size: u64) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse::<u64>().ok().map(|pages| pages * page_size)
}

/// `  12345 kB` → bytes. Pure, for tests.
#[cfg(any(target_os = "linux", test))]
fn parse_kb_value(rest: &str) -> Option<u64> {
    rest.split_whitespace().next()?.parse::<u64>().ok().map(|kb| kb * 1024)
}

pub fn fmt_mb(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) => format!("{}MB", b / (1024 * 1024)),
        None => "?MB".to_string(),
    }
}

// ---------------------------------------------------------------- logging

/// One line: `2026-…Z LEVEL target: message`. Emitted lines are written and
/// flushed immediately; filtered-out levels cost one atomic load.
pub fn log(level: u8, target: &str, message: &str) {
    if !enabled(level) {
        return;
    }
    let line = format!("{} {} {}: {}\n", iso8601(now_ms()), level_name(level), target, truncate(message, 8192));
    if let Some(s) = state() {
        if let Ok(mut writer) = s.writer.lock() {
            if let Some(w) = writer.as_mut() {
                w.write_line(&line);
            }
        }
    }
    // No file yet (very early, or the `code` CLI): stderr is better than loss.
    #[cfg(debug_assertions)]
    eprint!("{line}");
}

pub fn error(target: &str, message: &str) {
    log(ERROR, target, message);
}
pub fn warn(target: &str, message: &str) {
    log(WARN, target, message);
}
pub fn info(target: &str, message: &str) {
    log(INFO, target, message);
}
/// Kept for call sites that only matter with verbose logging on.
#[allow(dead_code)]
pub fn debug(target: &str, message: &str) {
    log(DEBUG, target, message);
}

// ---------------------------------------------------------------- crashes

fn system_summary() -> String {
    format!(
        "winfer {} · {} {} · {} cpu · pid {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        std::process::id(),
    )
}

fn crash_block(kind: &str, detail: &str) -> String {
    let mem = memory_snapshot();
    let mut out = format!(
        "=== {} {} ===\n{}\nrss={} peak={} system_total={} system_available={}\n{}\n",
        iso8601(now_ms()),
        kind,
        system_summary(),
        fmt_mb(mem.rss_bytes),
        fmt_mb(mem.peak_rss_bytes),
        fmt_mb(mem.system_total_bytes),
        fmt_mb(mem.system_available_bytes),
        detail,
    );
    if let Some(s) = state() {
        if let Ok(samples) = s.samples.lock() {
            if !samples.is_empty() {
                out.push_str("memory timeline (rss):\n");
                for sample in samples.iter() {
                    out.push_str(&format!("  {} {}MB\n", iso8601(sample.at_ms), sample.rss / (1024 * 1024)));
                }
            }
        }
        if let Ok(crumbs) = s.breadcrumbs.lock() {
            if !crumbs.is_empty() {
                out.push_str("breadcrumbs:\n");
                for crumb in crumbs.iter() {
                    out.push_str(&format!("  {crumb}\n"));
                }
            }
        }
        if let Ok(gauges) = s.gauges.lock() {
            if !gauges.is_empty() {
                let mut pairs: Vec<_> = gauges.iter().collect();
                pairs.sort_by(|a, b| a.0.cmp(b.0));
                let joined: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                out.push_str(&format!("gauges: {}\n", joined.join(" ")));
            }
        }
    }
    out.push('\n');
    out
}

/// Appends a block to `crash.log` (rotated past 1 MiB) and mirrors a short
/// line into the main log. Safe to call from a panic hook.
fn write_crash(kind: &str, detail: &str) {
    let block = crash_block(kind, detail);
    if let Some(s) = state() {
        let path = s.dir.join("crash.log");
        if std::fs::metadata(&path).map_or(0, |m| m.len()) > CRASH_MAX_BYTES {
            rotate_files(&path, 1);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(block.as_bytes());
            let _ = file.flush();
        }
    }
    log(ERROR, "crash", &format!("{kind}: {}", truncate(detail, 400)));
}

/// Installs the panic hook (once). Release builds have no console, so the
/// panic, its backtrace and the diagnostics context go to `crash.log`; the
/// default hook still runs (stderr in dev, debugger, …).
pub fn install_panic_hook(log_dir: PathBuf) {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(|| "?".to_string(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".to_string()
        };
        let detail = format!(
            "panic: {payload}\nat: {location}\nthread: {}\nbacktrace:\n{}",
            std::thread::current().name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture(),
        );
        // If diagnostics were never initialised (e.g. `winfer code`), fall
        // back to the pre-diagnostics location so nothing is lost.
        if state().is_none() {
            let _ = std::fs::create_dir_all(&log_dir);
            let entry = format!("[{}] {}: {info}\n{}\n\n", now_ms() / 1000, system_summary(), std::backtrace::Backtrace::force_capture());
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_dir.join("crash.log")) {
                let _ = file.write_all(entry.as_bytes());
            }
        } else {
            write_crash("PANIC", &detail);
        }
        default(info);
    }));
    #[cfg(target_os = "windows")]
    install_native_crash_handler(log_dir);
}

/// Windows only: a native crash (access violation in WebView2/ConPTY, stack
/// overflow, …) never reaches Rust's panic hook. An unhandled-exception
/// filter writes a minidump (`crash-<ts>.dmp`, openable in WinDbg/VS) plus
/// the usual crash block, then lets the process die as it would have.
#[cfg(target_os = "windows")]
fn install_native_crash_handler(log_dir: PathBuf) {
    use std::sync::atomic::AtomicBool as AB;
    static IN_HANDLER: AB = AB::new(false);
    static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
    let _ = LOG_DIR.set(log_dir);

    unsafe extern "system" fn filter(info: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Diagnostics::Debug::{
            MiniDumpWithDataSegs, MiniDumpWriteDump, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

        if IN_HANDLER.swap(true, Ordering::SeqCst) {
            return 1; // EXCEPTION_EXECUTE_HANDLER: we are the crash.
        }
        let dir = LOG_DIR.get().cloned().unwrap_or_else(|| PathBuf::from("."));
        let stamp = now_ms();
        let code: u32 = if info.is_null() {
            0
        } else {
            unsafe { (*(*info).ExceptionRecord).ExceptionCode as u32 }
        };
        // Minidump first: it is the artifact that identifies native crashes.
        let dump_path = dir.join(format!("crash-{stamp}.dmp"));
        if let Ok(file) = std::fs::File::create(&dump_path) {
            use std::os::windows::io::AsRawHandle;
            let exc = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: unsafe { GetCurrentThreadId() },
                ExceptionPointers: info as *mut EXCEPTION_POINTERS,
                ClientPointers: 0,
            };
            unsafe {
                let _ = MiniDumpWriteDump(
                    GetCurrentProcess(),
                    GetCurrentProcessId(),
                    file.as_raw_handle() as HANDLE,
                    MiniDumpWithDataSegs,
                    &exc,
                    std::ptr::null(),
                    std::ptr::null(),
                );
            }
        }
        write_crash(
            "NATIVE CRASH",
            &format!("exception code: 0x{code:08x}\nminidump: {}", dump_path.display()),
        );
        1 // EXCEPTION_EXECUTE_HANDLER
    }

    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(filter));
    }
}

// ---------------------------------------------------------------- sessions

const SESSION_FILE: &str = "last-session.json";

fn write_session_marker(dir: &Path, clean_exit: bool, rss: u64) {
    let body = format!(
        "{{\"version\":\"{}\",\"startedMs\":{},\"updatedMs\":{},\"cleanExit\":{},\"rssBytes\":{}}}",
        env!("CARGO_PKG_VERSION"),
        state().map_or(0, |s| now_ms() - s.started.elapsed().as_millis()),
        now_ms(),
        clean_exit,
        rss,
    );
    let _ = std::fs::write(dir.join(SESSION_FILE), body);
}

/// True when the previous run left an unmarked session file behind.
fn previous_run_crashed(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join(SESSION_FILE)) else {
        return false;
    };
    !text.contains("\"cleanExit\":true")
}

/// Starts file logging, the panic/native crash hooks and the memory sampler.
/// Idempotent; safe to call again from Tauri setup with the real data dir.
pub fn init(app_data_dir: &Path) {
    let dir = app_data_dir.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    install_panic_hook(dir.clone());

    if STATE.get().is_some() {
        return;
    }
    if let Ok(level) = std::env::var("WINFER_LOG") {
        LEVEL.store(parse_level(&level), Ordering::Relaxed);
    }
    let verbose_flag = dir.join("verbose.flag").exists();
    VERBOSE.store(verbose_flag, Ordering::Relaxed);
    if verbose_flag {
        LEVEL.store(DEBUG, Ordering::Relaxed);
    }
    let crashed_before = previous_run_crashed(&dir);
    let state_ = State_ {
        writer: Mutex::new(Some(Writer::open(&dir))),
        breadcrumbs: Mutex::new(VecDeque::new()),
        samples: Mutex::new(VecDeque::new()),
        gauges: Mutex::new(HashMap::new()),
        started: Instant::now(),
        dir: dir.clone(),
        previous_unexpected_exit: crashed_before,
        last_rss: AtomicU64::new(0),
        peak_rss: AtomicU64::new(0),
    };
    if STATE.set(state_).is_err() {
        return;
    }
    let mem = memory_snapshot();
    info(
        "app",
        &format!(
            "{} · log level {} · rss={}",
            system_summary(),
            level_name(LEVEL.load(Ordering::Relaxed)),
            fmt_mb(mem.rss_bytes),
        ),
    );
    if crashed_before {
        warn(
            "app",
            "previous run ended without a clean exit (crash, OOM kill or power loss); see the memory timeline above its end and crash.log",
        );
        breadcrumb("previous run ended unexpectedly".to_string());
    }
    write_session_marker(&dir, false, mem.rss_bytes.unwrap_or(0));
    spawn_sampler(dir);
}

/// The sampler: every 5 s record RSS; log a timeline line every 30 s, and
/// immediately when RSS jumps (>100 MiB in 30 s) or crosses 1/2/4 GiB — the
/// slope and the last values before a kill are what identify a leak.
fn spawn_sampler(dir: PathBuf) {
    std::thread::Builder::new()
        .name("diagnostics-memory".into())
        .spawn(move || {
            let mut last_logged_rss: u64 = 0;
            let mut last_timeline = Instant::now();
            let mut crossings: u64 = 0; // highest GiB threshold already reported
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let Some(s) = state() else { return };
                let snap = memory_snapshot();
                let Some(rss) = snap.rss_bytes else { continue };
                s.last_rss.store(rss, Ordering::Relaxed);
                s.peak_rss.fetch_max(rss, Ordering::Relaxed);
                if let Ok(mut samples) = s.samples.lock() {
                    if samples.len() >= MAX_SAMPLES {
                        samples.pop_front();
                    }
                    samples.push_back(Sample { at_ms: now_ms(), rss });
                }
                let gib = rss / (1024 * 1024 * 1024);
                if gib > crossings {
                    crossings = gib;
                    warn("memory", &format!("rss crossed {}GiB (now {})", gib, fmt_mb(Some(rss))));
                } else if last_logged_rss > 0 && rss > last_logged_rss + 100 * 1024 * 1024 && last_timeline.elapsed() > Duration::from_secs(30) {
                    warn(
                        "memory",
                        &format!("rss jumped {} → {} in <30s", fmt_mb(Some(last_logged_rss)), fmt_mb(Some(rss))),
                    );
                }
                if last_timeline.elapsed() >= Duration::from_secs(30) {
                    last_timeline = Instant::now();
                    last_logged_rss = rss;
                    let gauges = s
                        .gauges
                        .lock()
                        .map(|g| {
                            let mut pairs: Vec<_> = g.iter().collect();
                            pairs.sort_by(|a, b| a.0.cmp(b.0));
                            pairs.iter().map(|(k, v)| format!(" {k}={v}")).collect::<String>()
                        })
                        .unwrap_or_default();
                    info(
                        "memory",
                        &format!(
                            "rss={} peak={} system_available={}{}",
                            fmt_mb(Some(rss)),
                            fmt_mb(Some(s.peak_rss.load(Ordering::Relaxed))),
                            fmt_mb(snap.system_available_bytes),
                            gauges,
                        ),
                    );
                    write_session_marker(&dir, false, rss);
                }
            }
        })
        .ok();
}

/// Called on a normal app exit so the next start doesn't cry wolf.
pub fn mark_clean_exit() {
    if let Some(s) = state() {
        info("app", &format!("clean exit after {}s", s.started.elapsed().as_secs()));
        write_session_marker(&s.dir, true, s.last_rss.load(Ordering::Relaxed));
    }
}

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
    LEVEL.store(if on { DEBUG } else { INFO }, Ordering::Relaxed);
    if let Some(s) = state() {
        let flag = s.dir.join("verbose.flag");
        if on {
            let _ = std::fs::write(&flag, "1");
        } else {
            let _ = std::fs::remove_file(&flag);
        }
    }
    info("app", &format!("verbose logging {}", if on { "on" } else { "off" }));
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendEntry {
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Frontend log entries (batched by `logger.ts`). Lengths are capped here as
/// well: a buggy caller must not be able to flood the log it reports into.
#[tauri::command]
pub fn diagnostics_frontend_log(entries: Vec<FrontendEntry>) {
    for entry in entries.into_iter().take(100) {
        log(parse_level(&entry.level), &format!("web/{}", truncate(&entry.target, 64)), &truncate(&entry.message, 4096));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub log_dir: String,
    pub log_bytes: u64,
    pub crash_bytes: u64,
    pub dump_count: usize,
    pub memory: MemorySnapshot,
    pub session_peak_rss_bytes: u64,
    pub uptime_secs: u64,
    pub pty_sessions: usize,
    pub scrollback_bytes: usize,
    pub previous_unexpected_exit: bool,
    pub verbose: bool,
    pub crash_tail: String,
    pub recent_memory: Vec<String>,
}

fn dir_size(dir: &Path, pattern: &str) -> (u64, usize) {
    let (mut bytes, mut count) = (0u64, 0usize);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(pattern) {
                bytes += entry.metadata().map_or(0, |m| m.len());
                count += 1;
            }
        }
    }
    (bytes, count)
}

#[tauri::command]
pub fn diagnostics_info(app: AppHandle) -> DiagnosticsInfo {
    let (log_dir, uptime, prev, peak) = match state() {
        Some(s) => (
            s.dir.clone(),
            s.started.elapsed().as_secs(),
            s.previous_unexpected_exit,
            s.peak_rss.load(Ordering::Relaxed),
        ),
        None => (PathBuf::new(), 0, false, 0),
    };
    let (log_bytes, _) = dir_size(&log_dir, "winfer.log");
    let (crash_bytes, _) = dir_size(&log_dir, "crash.log");
    let (_, dump_count) = dir_size(&log_dir, ".dmp");
    let crash_tail = std::fs::read_to_string(log_dir.join("crash.log"))
        .map(|text| {
            let tail: Vec<&str> = text.lines().rev().take(60).collect();
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        })
        .unwrap_or_default();
    let recent_memory = state()
        .and_then(|s| s.samples.lock().ok())
        .map(|samples| {
            samples
                .iter()
                .rev()
                .take(24)
                .map(|s| format!("{} {}MB", iso8601(s.at_ms), s.rss / (1024 * 1024)))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        })
        .unwrap_or_default();
    DiagnosticsInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        log_dir: log_dir.to_string_lossy().into_owned(),
        log_bytes,
        crash_bytes,
        dump_count,
        memory: memory_snapshot(),
        session_peak_rss_bytes: peak,
        uptime_secs: uptime,
        pty_sessions: app.state::<crate::pty::PtyManager>().session_count(),
        scrollback_bytes: app.state::<crate::pty::PtyManager>().scrollback_bytes(),
        previous_unexpected_exit: prev,
        verbose: verbose(),
        crash_tail,
        recent_memory,
    }
}

#[tauri::command]
pub fn diagnostics_set_verbose(on: bool) {
    set_verbose(on);
}

/// Zips the logs plus a `summary.txt` (versions, memory, gauges, crash tail)
/// to `path` for attaching to a bug report. No app data, no secrets.
#[tauri::command]
pub async fn diagnostics_export(app: AppHandle, path: String) -> Result<String, String> {
    let info = diagnostics_info(app);
    tauri::async_runtime::spawn_blocking(move || {
        use zip::write::SimpleFileOptions;
        let dir = PathBuf::from(&info.log_dir);
        let file = std::fs::File::create(&path).map_err(|e| format!("can't create {path}: {e}"))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let summary = format!(
            "{}\nuptime: {}s\npty sessions: {}\nprevious unexpected exit: {}\nrss: {} (session peak {})\nsystem: {} total, {} available\nverbose: {}\n\nrecent memory (oldest first):\n{}\n\ncrash.log tail:\n{}",
            system_summary(),
            info.uptime_secs,
            info.pty_sessions,
            info.previous_unexpected_exit,
            fmt_mb(info.memory.rss_bytes),
            fmt_mb(Some(info.session_peak_rss_bytes)),
            fmt_mb(info.memory.system_total_bytes),
            fmt_mb(info.memory.system_available_bytes),
            info.verbose,
            info.recent_memory.join("\n"),
            info.crash_tail,
        );
        zip.start_file("summary.txt", options).map_err(|e| e.to_string())?;
        zip.write_all(summary.as_bytes()).map_err(|e| e.to_string())?;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                // Dumps can be hundreds of MB; name them, don't ship them.
                if name.ends_with(".dmp") {
                    continue;
                }
                if let Ok(data) = std::fs::read(entry.path()) {
                    // Cap each file so a runaway log can't bloat the bundle.
                    let data = if data.len() > 2 * 1024 * 1024 { &data[data.len() - 2 * 1024 * 1024..] } else { &data[..] };
                    zip.start_file(&name, options).map_err(|e| e.to_string())?;
                    zip.write_all(data).map_err(|e| e.to_string())?;
                }
            }
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(format!("diagnostics exported to {path}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Deletes the log/crash files (the running log reopens empty). Keeps dumps.
#[tauri::command]
pub fn diagnostics_clear() -> Result<(), String> {
    let Some(s) = state() else { return Ok(()) };
    if let Ok(mut writer) = s.writer.lock() {
        *writer = None;
    }
    for name in ["winfer.log", "winfer.log.1", "winfer.log.2", "winfer.log.3", "crash.log", "crash.log.1"] {
        let _ = std::fs::remove_file(s.dir.join(name));
    }
    if let Ok(mut writer) = s.writer.lock() {
        *writer = Some(Writer::open(&s.dir));
    }
    info("app", "logs cleared from Settings");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_formats_epoch_and_known_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(iso8601(1_767_225_600_000), "2026-01-01T00:00:00.000Z");
    }

    #[test]
    fn levels_filter_and_parse() {
        assert_eq!(parse_level("error"), ERROR);
        assert_eq!(parse_level("WARN"), WARN);
        assert_eq!(parse_level("debug"), DEBUG);
        assert_eq!(parse_level("nonsense"), INFO);
        assert!(ERROR < WARN && WARN < INFO && INFO < DEBUG);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("aébcdef", 2), "a…");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_proc_memory_formats() {
        assert_eq!(parse_statm_rss("1000 250 3 4 5 6 7", 4096), Some(250 * 4096));
        assert_eq!(parse_statm_rss("garbage", 4096), None);
        assert_eq!(parse_kb_value("  2048 kB"), Some(2048 * 1024));
    }

    #[test]
    fn rotation_keeps_backups() {
        let dir = std::env::temp_dir().join(format!("winfer-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("winfer.log");
        std::fs::write(&log, "current").unwrap();
        rotate_files(&log, 3);
        std::fs::write(&log, "newer").unwrap();
        rotate_files(&log, 3);
        assert_eq!(std::fs::read_to_string(dir.join("winfer.log.1")).unwrap(), "newer");
        assert_eq!(std::fs::read_to_string(dir.join("winfer.log.2")).unwrap(), "current");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn init_writes_log_file_and_detects_unclean_previous_run() {
        let dir = std::env::temp_dir().join(format!("winfer-diag-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init(&dir);
        info("test", "hello diagnostics");
        let log = std::fs::read_to_string(dir.join("logs/winfer.log")).unwrap();
        assert!(log.contains("INFO app: winfer"), "startup line: {log}");
        assert!(log.contains("INFO test: hello diagnostics"), "log line: {log}");
        // The marker starts unclean; only mark_clean_exit flips it.
        assert!(previous_run_crashed(&dir.join("logs")));
        mark_clean_exit();
        assert!(!previous_run_crashed(&dir.join("logs")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_block_carries_context() {
        let block = crash_block("PANIC", "panic: boom\nat: src/main.rs:1:1");
        assert!(block.contains("PANIC"), "{block}");
        assert!(block.contains("panic: boom"), "{block}");
        assert!(block.contains("rss="), "{block}");
        assert!(block.contains("winfer "), "{block}");
    }

    #[test]
    fn writer_rotates_and_appends() {
        let dir = std::env::temp_dir().join(format!("winfer-diagw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = Writer::open(&dir);
        w.size = LOG_MAX_BYTES + 1; // force rotation on next line
        w.write_line("hello\n");
        drop(w);
        assert_eq!(std::fs::read_to_string(dir.join("winfer.log")).unwrap(), "hello\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
