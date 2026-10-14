//! How much of their plan's usage limits the agents have left.
//!
//! - Claude Code hands its status line command the account's rate limits (5-hour and weekly
//!   windows). winfer registers `winfer statusline` as the status line: it reports them to the
//!   app and then runs the user's own status line, so that keeps working.
//! - Codex records its rate limits after every turn in its session logs (`~/.codex/sessions`).
//! - OpenCode has no plan limits of its own (it uses whatever model providers are set up), so
//!   it reports today's tokens and cost.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// "session" (5-hour), "week", or another window's id.
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    /// Unix time in milliseconds.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub provider: String,
    pub windows: Vec<Window>,
    /// When the numbers were measured (unix ms).
    pub at: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Claude Code ------------------------------------------------------------------------------------

const CLAUDE_WINDOWS: [(&str, &str, &str); 4] = [
    ("five_hour", "session", "5-hour"),
    ("seven_day", "week", "Weekly"),
    ("seven_day_opus", "week-opus", "Weekly (Opus)"),
    ("seven_day_sonnet", "week-sonnet", "Weekly (Sonnet)"),
];

/// The windows in a status line input's `rate_limits` (null for API-key sessions).
pub fn claude_windows(input: &Value) -> Vec<Window> {
    let limits = &input["rate_limits"];
    CLAUDE_WINDOWS
        .iter()
        .filter_map(|(key, kind, label)| {
            let w = &limits[*key];
            let used = w["used_percentage"].as_f64().or_else(|| w["utilization"].as_f64())?;
            Some(Window {
                kind: kind.to_string(),
                label: label.to_string(),
                used_percent: used,
                resets_at: seconds_or_date(&w["resets_at"]),
            })
        })
        .collect()
}

/// `resets_at` comes as unix seconds or as an ISO date.
fn seconds_or_date(v: &Value) -> Option<i64> {
    v.as_f64().map(|s| (s * 1000.0) as i64).or_else(|| v.as_str().and_then(parse_rfc3339))
}

/// Entry point for `winfer statusline`, Claude Code's status line command inside winfer:
/// reports the rate limits, then prints whatever the user's own status line prints.
pub fn run_statusline() {
    let mut input = String::new();
    let _ = std::io::stdin().take(1024 * 1024).read_to_string(&mut input);
    let parsed: Value = serde_json::from_str(&input).unwrap_or(Value::Null);

    let windows = claude_windows(&parsed);
    let report = (!windows.is_empty()).then(|| {
        let body = json!({ "windows": windows }).to_string();
        std::thread::spawn(move || {
            let var = |k: &str| std::env::var(k).unwrap_or_default();
            crate::hooks::send_hook(&var("WINFER_HOOK"), &var("WINFER_TOKEN"), &var("WINFER_PANE_ID"), "claude", "usage", body);
        })
    });

    let cwd = parsed["workspace"]["current_dir"].as_str().or_else(|| parsed["cwd"].as_str()).map(PathBuf::from);
    if let Some(command) = user_statusline(cwd.as_deref()) {
        if let Some(out) = run_shell(&command, &input) {
            let _ = std::io::stdout().write_all(&out);
        }
    }
    if let Some(report) = report {
        let _ = report.join();
    }
}

/// The status line command the user configured (project settings first, like Claude Code).
fn user_statusline(cwd: Option<&Path>) -> Option<String> {
    let mut files = Vec::new();
    if let Some(dir) = cwd {
        files.push(dir.join(".claude/settings.local.json"));
        files.push(dir.join(".claude/settings.json"));
    }
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")));
    if let Some(dir) = config {
        files.push(dir.join("settings.json"));
    }
    files.iter().find_map(|f| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?;
        let line = &v["statusLine"];
        (line["type"].as_str() == Some("command")).then(|| line["command"].as_str().map(String::from)).flatten()
    })
}

fn run_shell(command: &str, input: &str) -> Option<Vec<u8>> {
    let mut cmd = if cfg!(windows) {
        match crate::env::find_command("bash") {
            Some(bash) => {
                let mut c = Command::new(bash);
                c.args(["-c", command]);
                c
            }
            None => {
                let mut c = Command::new("powershell");
                c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
                c
            }
        }
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let mut child = crate::env::no_window(cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())).spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    child.wait_with_output().ok().map(|o| o.stdout)
}

/// The status line entry for winfer's Claude settings file.
pub fn claude_statusline_setting(exe: &str) -> Value {
    // Claude runs it through a shell; quote the path if it has spaces.
    let program = if exe.contains(' ') { format!("\"{exe}\"") } else { exe.to_string() };
    json!({ "type": "command", "command": format!("{program} statusline"), "padding": 0 })
}

// Codex -----------------------------------------------------------------------------------------

fn codex_sessions() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
    Some(home.join("sessions"))
}

/// Session logs, newest first (sessions/YYYY/MM/DD/rollout-*.jsonl).
fn newest_logs(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let sorted_dirs = |dir: &Path| {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        entries.sort();
        entries.reverse();
        entries
    };
    'outer: for year in sorted_dirs(root) {
        for month in sorted_dirs(&year) {
            for day in sorted_dirs(&month) {
                let mut logs: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&day)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                    .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
                    .collect();
                logs.sort_by(|a, b| b.0.cmp(&a.0));
                files.extend(logs.into_iter().map(|(_, p)| p));
                if files.len() >= max {
                    break 'outer;
                }
            }
        }
    }
    files.truncate(max);
    files
}

/// The end of a file (rate limit events are near it); the first, likely partial, line dropped.
fn tail(path: &Path, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if start > 0 { text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default() } else { text })
}

fn window_label(minutes: i64) -> (String, String) {
    match minutes {
        300 => ("session".into(), "5-hour".into()),
        10080 => ("week".into(), "Weekly".into()),
        m if m % 1440 == 0 => (format!("{}d", m / 1440), format!("{}-day", m / 1440)),
        m if m % 60 == 0 => (format!("{}h", m / 60), format!("{}-hour", m / 60)),
        m => (format!("{m}m"), format!("{m}-minute")),
    }
}

/// The last rate limit snapshot in a Codex session log.
pub fn codex_snapshot_in(log: &str) -> Option<Snapshot> {
    log.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        let at = v["timestamp"].as_str().and_then(parse_rfc3339).unwrap_or_else(now_ms);
        let limits = &v["payload"]["rate_limits"];
        let windows: Vec<Window> = ["primary", "secondary"]
            .iter()
            .filter_map(|key| {
                let w = &limits[*key];
                let used = w["used_percent"].as_f64()?;
                let minutes = w["window_minutes"].as_i64().unwrap_or(0);
                let resets_at = seconds_or_date(&w["resets_at"]).or_else(|| w["resets_in_seconds"].as_i64().map(|s| at + s * 1000));
                let (kind, label) = window_label(minutes);
                Some(Window { kind, label, used_percent: used, resets_at })
            })
            .collect();
        (!windows.is_empty()).then(|| Snapshot { provider: "codex".into(), windows, at })
    })
}

#[tauri::command]
pub async fn usage_codex() -> Option<Snapshot> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = codex_sessions()?;
        newest_logs(&root, 4).iter().find_map(|log| codex_snapshot_in(&tail(log, 512 * 1024)?))
    })
    .await
    .ok()
    .flatten()
}

// OpenCode --------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spend {
    pub tokens: u64,
    pub cost: f64,
    pub sessions: u64,
}

/// Today's tokens and cost from `opencode stats` (OpenCode 2; older versions return None).
#[tauri::command]
pub async fn usage_opencode() -> Option<Spend> {
    tauri::async_runtime::spawn_blocking(|| {
        let launcher = crate::env::launcher("opencode")?;
        let out = crate::env::no_window(
            Command::new(&launcher.program)
                .args(&launcher.args)
                .args(["stats", "--days", "0", "--cost", "--json"])
                .env("PATH", crate::env::path_var())
                .stdin(Stdio::null())
                .stderr(Stdio::null()),
        )
        .output()
        .ok()?;
        let v: Value = serde_json::from_slice(&out.stdout).ok()?;
        let t = &v["tokens"];
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        Some(Spend {
            tokens: n(&t["input"]) + n(&t["output"]) + n(&t["reasoning"]),
            cost: v["cost"].as_f64().unwrap_or(0.0),
            sessions: n(&v["sessions"]),
        })
    })
    .await
    .ok()
    .flatten()
}

// Dates -----------------------------------------------------------------------------------------

/// "2026-10-14T00:00:29.123Z" (or with an offset) → unix ms.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = &s[19..];
    let (frac, zone) = match rest.strip_prefix('.') {
        Some(r) => {
            let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            let digits = &r[..end.min(3)];
            let ms = format!("{digits:0<3}").parse::<i64>().ok()?;
            (ms, &r[end..])
        }
        None => (0, rest),
    };
    let offset = match zone {
        "" | "Z" | "z" => 0,
        z => {
            let sign = if z.starts_with('-') { -1 } else { 1 };
            let hh: i64 = z.get(1..3)?.parse().ok()?;
            let mm: i64 = z.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(0);
            sign * (hh * 60 + mm)
        }
    };
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 86400 + h * 3600 + mi * 60 + sec - offset * 60) * 1000) + frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dates() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-14T00:00:29.5Z"), Some(1791936029500));
        assert_eq!(parse_rfc3339("2026-10-14T02:00:29+02:00"), Some(1791936029000));
    }

    #[test]
    fn reads_claude_status_line_limits() {
        let input = json!({ "rate_limits": {
            "five_hour": { "used_percentage": 38.0, "resets_at": 1791936000 },
            "seven_day": { "used_percentage": 69.5, "resets_at": 1792500000 },
        }});
        let w = claude_windows(&input);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].kind, "session");
        assert_eq!(w[0].resets_at, Some(1791936000000));
        assert_eq!(w[1].used_percent, 69.5);
        assert!(claude_windows(&json!({ "rate_limits": null })).is_empty());
    }

    #[test]
    fn reads_the_last_codex_rate_limits() {
        let log = [
            r#"{"timestamp":"2026-10-14T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1791929188},"secondary":{"used_percent":1.0,"window_minutes":10080,"resets_at":1792515988}}}}"#,
            r#"{"timestamp":"2026-10-14T00:10:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":12.0,"window_minutes":300,"resets_at":1791929188},"secondary":{"used_percent":3.0,"window_minutes":10080,"resets_at":1792515988}}}}"#,
            r#"{"timestamp":"2026-10-14T00:11:00.000Z","type":"response_item","payload":{}}"#,
        ]
        .join("\n");
        let s = codex_snapshot_in(&log).unwrap();
        assert_eq!(s.at, parse_rfc3339("2026-10-14T00:10:00.000Z").unwrap());
        assert_eq!(s.windows[0], Window { kind: "session".into(), label: "5-hour".into(), used_percent: 12.0, resets_at: Some(1791929188000) });
        assert_eq!(s.windows[1].kind, "week");
    }

    /// Reads this machine's real Codex logs: `cargo test codex_logs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn codex_logs_on_this_machine() {
        let root = codex_sessions().unwrap();
        let snapshot = newest_logs(&root, 4).iter().find_map(|log| codex_snapshot_in(&tail(log, 512 * 1024)?));
        println!("{snapshot:#?}");
    }
}
