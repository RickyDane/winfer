//! Browser diagnostics: `browser.log` in the app data folder (next to `crash.log`), so flaky
//! tabs can be understood after the fact. Repeats of the same line (an input event failing on
//! every mouse move) are folded into one "repeated N times" line.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The log is moved to `browser.log.1` past this size.
const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// How long an identical line counts as a repeat.
const REPEAT_WINDOW: Duration = Duration::from_secs(5);

struct Last {
    line: String,
    at: Instant,
    repeats: u32,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

pub fn path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("dev.winfer.app").join("browser.log"))
}

fn timestamp() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}

fn append(text: &str) {
    if cfg!(debug_assertions) {
        eprint!("{text}");
    }
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(text.as_bytes());
    }
}

/// Writes one line at `level` ("info", "warn", "error").
pub fn write(level: &str, message: &str) {
    let line = format!("{level} {message}");
    let mut last = LAST.lock().unwrap();
    if let Some(l) = last.as_mut().filter(|l| l.line == line && l.at.elapsed() < REPEAT_WINDOW) {
        l.repeats += 1;
        l.at = Instant::now();
        return;
    }
    let mut text = String::new();
    if let Some(l) = last.take().filter(|l| l.repeats > 0) {
        text.push_str(&format!("{} {} (repeated {} more times)\n", timestamp(), l.line, l.repeats));
    }
    text.push_str(&format!("{} {line}\n", timestamp()));
    *last = Some(Last {
        line,
        at: Instant::now(),
        repeats: 0,
    });
    append(&text);
}

macro_rules! info {
    ($($arg:tt)*) => { $crate::browser::log::write("info", &format!($($arg)*)) };
}
macro_rules! warning {
    ($($arg:tt)*) => { $crate::browser::log::write("warn", &format!($($arg)*)) };
}
pub(crate) use {info, warning};

/// Logs a command's error, tagged with what it was doing, and passes the result through.
pub fn check<T>(what: &str, result: Result<T, String>) -> Result<T, String> {
    if let Err(err) = &result {
        write("error", &format!("{what}: {err}"));
    }
    result
}

/// Lets the panel record what it saw go wrong (failed attaches, input that didn't arrive).
#[tauri::command]
pub fn browser_log(level: String, message: String) {
    let level = match level.as_str() {
        "error" | "warn" | "info" => level.as_str(),
        _ => "info",
    };
    write(level, &format!("ui: {message}"));
}
