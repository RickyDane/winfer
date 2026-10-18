//! How much of their plan's usage limits the agents have left.
//!
//! - Claude Code hands its status line command the account's rate limits (5-hour and weekly
//!   windows). winfer registers `winfer statusline` as the status line: it reports them to the
//!   app and then runs the user's own status line, so that keeps working.
//!   The app also polls Claude's account usage endpoint with Claude Code's existing sign-in,
//!   so limits stay current when Claude is used outside winfer or status reports stop.
//! - Codex: asked live from ChatGPT's usage endpoint with Codex's own sign-in
//!   (`~/.codex/auth.json`), the numbers `/status` shows. Falls back to the rate limits Codex
//!   records after every turn in its session logs (`~/.codex/sessions`), which go stale when
//!   Codex is used elsewhere (another machine, the IDE extension, the cloud).
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

/// How full an agent session's context window is.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub used_percent: f64,
    /// Tokens in the context now.
    pub tokens: u64,
    /// The model's context window.
    pub window: u64,
    /// Unix ms.
    pub at: i64,
    /// Where the session's conversation is kept, for the breakdown (Claude's transcript, Codex's
    /// session log, OpenCode's session id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

// Claude Code ------------------------------------------------------------------------------------

/// The status line input's `context_window`.
pub fn claude_context(input: &Value) -> Option<Context> {
    let c = &input["context_window"];
    let window = c["context_window_size"].as_u64().filter(|w| *w > 0)?;
    let tokens = c["total_input_tokens"].as_u64().unwrap_or(0);
    let used_percent = c["used_percentage"].as_f64().unwrap_or(tokens as f64 * 100.0 / window as f64);
    let source = input["transcript_path"].as_str().filter(|p| !p.is_empty()).map(String::from);
    Some(Context { used_percent, tokens, window, at: now_ms(), source })
}

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

/// Read Claude Code's existing sign-in without rotating or persisting its tokens.
fn claude_token(auth: &Value, now: i64) -> Option<String> {
    let oauth = &auth["claudeAiOauth"];
    if oauth["expiresAt"].as_i64().is_some_and(|at| at <= now) {
        return None;
    }
    oauth["accessToken"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn claude_credentials() -> Option<Value> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    // macOS Claude Code keeps its sign-in in Keychain; other platforms use this file.
    #[cfg(target_os = "macos")]
    {
        use sha2::{Digest, Sha256};
        let suffix = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(|_| format!("-{}", &hex::encode(Sha256::digest(config.to_string_lossy().as_bytes()))[..8]))
            .unwrap_or_default();
        let service = format!("Claude Code{suffix}-credentials");
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", &service, "-w"])
            .stdin(Stdio::null()).stderr(Stdio::null()).output().ok();
        if let Some(out) = out.filter(|o| o.status.success()) {
            if let Ok(auth) = serde_json::from_slice(&out.stdout) {
                return Some(auth);
            }
        }
    }
    serde_json::from_str(&std::fs::read_to_string(config.join(".credentials.json")).ok()?).ok()
}

fn claude_usage_snapshot(body: &Value, at: i64) -> Option<Snapshot> {
    let windows = claude_windows(&json!({ "rate_limits": body }));
    (!windows.is_empty()).then(|| Snapshot { provider: "claude".into(), windows, at })
}

/// Account-wide limits, including use outside winfer. Errors keep the previous reading visible.
#[tauri::command]
pub async fn usage_claude() -> Result<Snapshot, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let token = claude_credentials().and_then(|a| claude_token(&a, now_ms()))
            .ok_or("Claude sign-in unavailable or expired. Sign in through Claude Code, then refresh.")?;
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).build().into();
        let mut response = agent.get("https://api.anthropic.com/api/oauth/usage")
            .header("Authorization", &format!("Bearer {token}"))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("User-Agent", "claude-cli")
            .call().map_err(|_| "Couldn't refresh Claude limits. Check your connection and Claude Code sign-in.")?;
        let body: Value = response.body_mut().read_json().map_err(|_| "Claude returned an invalid usage response.")?;
        claude_usage_snapshot(&body, now_ms()).ok_or_else(|| "Claude returned no plan limits.".to_string())
    }).await.map_err(|_| "Claude usage refresh failed.".to_string())?
}

/// Entry point for `winfer statusline`, Claude Code's status line command inside winfer:
/// reports the rate limits, then prints whatever the user's own status line prints.
pub fn run_statusline() {
    let mut input = String::new();
    let _ = std::io::stdin().take(1024 * 1024).read_to_string(&mut input);
    let parsed: Value = serde_json::from_str(&input).unwrap_or(Value::Null);

    let windows = claude_windows(&parsed);
    let context = claude_context(&parsed);
    let report = (!windows.is_empty() || context.is_some()).then(|| {
        let body = json!({ "windows": windows, "context": context }).to_string();
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

fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
}

fn codex_sessions() -> Option<PathBuf> {
    Some(codex_home()?.join("sessions"))
}

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// The account's limits right now, from the endpoint Codex's `/status` reads. Needs a ChatGPT
/// sign-in (API-key users have no plan limits); None when signed out or the token has expired
/// (Codex refreshes it on its next run).
/// Falls back to OpenCode's ChatGPT sign-in: the same plan limits, used through OpenCode.
fn codex_live() -> Option<Snapshot> {
    let codex = codex_home()
        .and_then(|home| std::fs::read_to_string(home.join("auth.json")).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|auth| {
            let token = auth["tokens"]["access_token"].as_str().filter(|t| !t.is_empty())?.to_string();
            Some((token, auth["tokens"]["account_id"].as_str().map(String::from)))
        });
    let (token, account) = codex.or_else(opencode_chatgpt_token)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).build().into();
    let mut req = agent.get(CODEX_USAGE_URL).header("Authorization", &format!("Bearer {token}")).header("User-Agent", "codex-cli");
    if let Some(account) = account {
        req = req.header("ChatGPT-Account-Id", &account);
    }
    let body: Value = req.call().ok()?.body_mut().read_json().ok()?;
    codex_usage_snapshot(&body, now_ms())
}

/// The usage endpoint's answer as a snapshot.
pub fn codex_usage_snapshot(body: &Value, at: i64) -> Option<Snapshot> {
    let limits = &body["rate_limit"];
    let mut windows: Vec<Window> = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| {
            let w = &limits[*key];
            let used = w["used_percent"].as_f64()?;
            let minutes = w["limit_window_seconds"].as_i64().unwrap_or(0) / 60;
            let resets_at = seconds_or_date(&w["reset_at"]).or_else(|| w["reset_after_seconds"].as_i64().map(|s| at + s * 1000));
            let (kind, label) = window_label(minutes);
            Some(Window { kind, label, used_percent: used, resets_at })
        })
        .collect();
    windows.extend(credit_window(&body["spend_control"]["individual_limit"]));
    (!windows.is_empty()).then(|| Snapshot { provider: "codex".into(), windows, at })
}

/// Workspace plans: the member's monthly credit limit.
fn credit_window(credit: &Value) -> Option<Window> {
    let left = credit["remaining_percent"].as_f64()?;
    Some(Window {
        kind: "month-credit".into(),
        label: "Monthly credit".into(),
        used_percent: 100.0 - left,
        resets_at: seconds_or_date(&credit["resets_at"]),
    })
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

/// The last rate limit snapshot in a Codex session log. Codex logs several limit buckets
/// (`codex`, `codex_other`, per-model ones); only the main `codex` bucket is the plan's limits,
/// the one `/status` shows (no `limit_id` means `codex`).
pub fn codex_snapshot_in(log: &str) -> Option<Snapshot> {
    log.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        let at = v["timestamp"].as_str().and_then(parse_rfc3339).unwrap_or_else(now_ms);
        let limits = &v["payload"]["rate_limits"];
        if limits["limit_id"].as_str().is_some_and(|id| id != "codex") {
            return None;
        }
        let mut windows: Vec<Window> = ["primary", "secondary"]
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
        windows.extend(credit_window(&limits["individual_limit"]));
        (!windows.is_empty()).then(|| Snapshot { provider: "codex".into(), windows, at })
    })
}

/// A session's context use at its last turn (from the newest `token_count` event).
pub fn codex_context_in(log: &str) -> Option<Context> {
    log.lines().rev().filter(|l| l.contains("\"model_context_window\"")).find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        let info = &v["payload"]["info"];
        let window = info["model_context_window"].as_u64().filter(|w| *w > 0)?;
        let tokens = info["last_token_usage"]["total_tokens"].as_u64()?;
        let at = v["timestamp"].as_str().and_then(parse_rfc3339).unwrap_or_else(now_ms);
        Some(Context { used_percent: tokens as f64 * 100.0 / window as f64, tokens, window, at, source: None })
    })
}

/// The session log for a pane: by session id (Codex's `thread-id`, the end of the log's file
/// name) when known, else the newest session started in `cwd`.
fn codex_log_for(root: &Path, session: Option<&str>, cwd: &str) -> Option<PathBuf> {
    let logs = newest_logs(root, 40);
    if let Some(id) = session.filter(|s| !s.is_empty()) {
        if let Some(log) = logs.iter().find(|p| p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.ends_with(id))) {
            return Some(log.clone());
        }
    }
    let wanted = Path::new(cwd);
    logs.into_iter().find(|log| {
        let Ok(file) = std::fs::File::open(log) else { return false };
        let mut first = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(file.take(64 * 1024)), &mut first);
        serde_json::from_str::<Value>(&first).ok().and_then(|v| v["payload"]["cwd"].as_str().map(|c| Path::new(c) == wanted)).unwrap_or(false)
    })
}

#[tauri::command]
pub async fn usage_codex_context(session: Option<String>, cwd: String) -> Option<Context> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = codex_sessions()?;
        let log = codex_log_for(&root, session.as_deref(), &cwd)?;
        let context = codex_context_in(&tail(&log, 1024 * 1024)?)?;
        Some(Context { source: Some(log.to_string_lossy().into_owned()), ..context })
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
pub async fn usage_codex() -> Option<Snapshot> {
    tauri::async_runtime::spawn_blocking(|| {
        if let Some(live) = codex_live() {
            return Some(live);
        }
        let root = codex_sessions()?;
        // The newest reading of the last few sessions (a session started yesterday may still be
        // running, so folder order alone isn't enough).
        newest_logs(&root, 8).iter().filter_map(|log| codex_snapshot_in(&tail(log, 512 * 1024)?)).max_by_key(|s| s.at)
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

fn opencode_data() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))?;
    Some(data.join("opencode"))
}

/// OpenCode's sign-ins per model provider (`auth.json`).
fn opencode_auth() -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(opencode_data()?.join("auth.json")).ok()?).ok()
}

/// Whether OpenCode signs in to a model provider with an account (a plan with usage limits,
/// like ChatGPT or Claude) rather than an API key.
fn opencode_signed_in(auth: &Value, provider: &str) -> bool {
    auth[provider]["type"].as_str() == Some("oauth")
}

/// OpenCode's ChatGPT access token and account, unless it has expired (OpenCode refreshes it
/// on its next run).
fn opencode_chatgpt_token() -> Option<(String, Option<String>)> {
    let auth = opencode_auth()?;
    let openai = &auth["openai"];
    if !opencode_signed_in(&auth, "openai") || openai["expires"].as_i64().is_some_and(|e| e <= now_ms()) {
        return None;
    }
    let token = openai["access"].as_str().filter(|t| !t.is_empty())?.to_string();
    Some((token, openai["accountId"].as_str().map(String::from)))
}

/// The model an OpenCode session uses and whether its provider is signed in with a plan.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeModel {
    pub provider: String,
    pub model: String,
    pub plan: bool,
}

/// The model of the newest OpenCode session (not a subagent's) started in `cwd`: its last
/// answer's model, else the session's.
fn opencode_model_at(conn: &rusqlite::Connection, cwd: &str) -> Option<Value> {
    let mut stmt = conn
        .prepare("select id, directory, model from session_v2 where parent_id is null order by time_updated desc limit 200")
        .ok()?;
    let (session, model): (String, Option<String>) = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))
        .ok()?
        .flatten()
        .find(|(_, dir, _)| same_dir(dir, cwd))
        .map(|(id, _, model)| (id, model))?;
    let mut stmt = conn
        .prepare("select data from session_message where session_id = ?1 and type = 'assistant' order by seq desc limit 1")
        .ok()?;
    let last = stmt
        .query_map([&session], |r| r.get::<_, String>(0))
        .ok()?
        .flatten()
        .find_map(|raw| serde_json::from_str::<Value>(&raw).ok().map(|d| d["model"].clone()).filter(Value::is_object));
    last.or_else(|| model.and_then(|m| serde_json::from_str(&m).ok()))
}

/// Which model the OpenCode session in a pane's folder uses (OpenCode 2; older versions: None).
#[tauri::command]
pub async fn usage_opencode_model(cwd: String) -> Option<OpencodeModel> {
    tauri::async_runtime::spawn_blocking(move || {
        let model = opencode_model_at(&opencode_db()?, &cwd)?;
        let provider = model["providerID"].as_str()?.to_string();
        let plan = opencode_auth().is_some_and(|auth| opencode_signed_in(&auth, &provider));
        Some(OpencodeModel { provider, model: model["id"].as_str().unwrap_or_default().into(), plan })
    })
    .await
    .ok()
    .flatten()
}

/// OpenCode 2's database (sessions, messages, the models catalog), read-only.
pub(crate) fn opencode_db() -> Option<rusqlite::Connection> {
    let path = opencode_data()?.join("opencode.db");
    if !path.exists() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    Some(conn)
}

fn same_dir(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim_end_matches(['/', '\\']).replace('\\', "/");
    if cfg!(windows) { norm(a).eq_ignore_ascii_case(&norm(b)) } else { norm(a) == norm(b) }
}

/// Tokens in the context after an assistant message: everything the model saw and wrote.
fn opencode_message_tokens(data: &Value) -> Option<u64> {
    let t = &data["tokens"];
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let total = n(&t["input"]) + n(&t["output"]) + n(&t["reasoning"]) + n(&t["cache"]["read"]) + n(&t["cache"]["write"]);
    (total > 0).then_some(total)
}

/// A model's context window from OpenCode's cached models.dev catalog. Parsed once (it's large).
fn opencode_model_window(conn: &rusqlite::Connection, provider: &str, model: &str) -> Option<u64> {
    static CATALOG: std::sync::Mutex<Option<std::collections::HashMap<String, u64>>> = std::sync::Mutex::new(None);
    let mut catalog = CATALOG.lock().ok()?;
    if catalog.is_none() {
        let raw: String = conn.query_row("select value from kv where key = 'models-dev:catalog'", [], |r| r.get(0)).ok()?;
        let outer: Value = serde_json::from_str(&raw).ok()?;
        let body: Value = match &outer["body"] {
            Value::String(s) => serde_json::from_str(s).ok()?,
            v => v.clone(),
        };
        let mut windows = std::collections::HashMap::new();
        for (pid, p) in body.as_object()? {
            for (mid, m) in p["models"].as_object().into_iter().flatten() {
                if let Some(ctx) = m["limit"]["context"].as_u64() {
                    windows.insert(format!("{pid}/{mid}"), ctx);
                }
            }
        }
        *catalog = Some(windows);
    }
    catalog.as_ref()?.get(&format!("{provider}/{model}")).copied()
}

/// Context use of the newest OpenCode session (not a subagent's) started in `cwd`.
fn opencode_context_at(conn: &rusqlite::Connection, cwd: &str) -> Option<Context> {
    let mut stmt = conn
        .prepare("select id, directory, model from session_v2 where parent_id is null order by time_updated desc limit 200")
        .ok()?;
    let (session, model): (String, Option<String>) = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))
        .ok()?
        .flatten()
        .find(|(_, dir, _)| same_dir(dir, cwd))
        .map(|(id, _, model)| (id, model))?;
    let mut stmt = conn
        .prepare("select data, time_updated from session_message where session_id = ?1 and type = 'assistant' order by seq desc limit 20")
        .ok()?;
    let (tokens, data, at) = stmt
        .query_map([&session], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .ok()?
        .flatten()
        .find_map(|(raw, at)| {
            let data: Value = serde_json::from_str(&raw).ok()?;
            Some((opencode_message_tokens(&data)?, data, at))
        })?;
    // The message names its model; the session's model is the fallback.
    let model = Some(data["model"].clone()).filter(|m| m.is_object()).or_else(|| model.and_then(|m| serde_json::from_str(&m).ok()))?;
    let window = opencode_model_window(conn, model["providerID"].as_str()?, model["id"].as_str()?)?;
    Some(Context { used_percent: tokens as f64 * 100.0 / window as f64, tokens, window, at, source: Some(session) })
}

/// Context use of the OpenCode session in a pane's folder (OpenCode 2; older versions: None).
#[tauri::command]
pub async fn usage_opencode_context(cwd: String) -> Option<Context> {
    tauri::async_runtime::spawn_blocking(move || opencode_context_at(&opencode_db()?, &cwd)).await.ok().flatten()
}

// Dates -----------------------------------------------------------------------------------------

/// "2026-10-14T00:00:29.123Z" (or with an offset) → unix ms.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i64> {
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
    fn reads_live_claude_limits_including_exhausted_session() {
        let body = json!({
            "five_hour": { "utilization": 100.0, "resets_at": "2026-10-14T02:00:29+02:00" },
            "seven_day": { "utilization": 17.0, "resets_at": "2026-10-20T00:00:00Z" },
            "seven_day_opus": null,
            "seven_day_sonnet": { "utilization": 0, "resets_at": null }
        });
        let s = claude_usage_snapshot(&body, 123).unwrap();
        assert_eq!((s.provider.as_str(), s.at), ("claude", 123));
        assert_eq!(s.windows.len(), 3);
        assert_eq!((s.windows[0].used_percent, s.windows[0].resets_at), (100.0, Some(1791936029000)));
        assert_eq!(s.windows[1].used_percent, 17.0);
        assert_eq!(s.windows[2].used_percent, 0.0);
        assert!(claude_usage_snapshot(&json!({}), 0).is_none());
    }

    #[test]
    fn reads_only_unexpired_claude_oauth_tokens() {
        let auth = json!({ "claudeAiOauth": { "accessToken": "test-token", "expiresAt": 100 } });
        assert_eq!(claude_token(&auth, 99).as_deref(), Some("test-token"));
        assert!(claude_token(&auth, 100).is_none());
        assert!(claude_token(&json!({ "apiKey": "test-key" }), 0).is_none());
        assert!(claude_token(&json!({ "claudeAiOauth": { "accessToken": "" } }), 0).is_none());
    }

    /// Read-only integration check; never prints credentials.
    #[test]
    #[ignore]
    fn claude_live_on_this_machine() {
        let result = tauri::async_runtime::block_on(usage_claude());
        assert!(result.is_ok(), "{}", result.unwrap_err());
    }

    #[test]
    fn reads_context_use() {
        let input = json!({ "context_window": { "total_input_tokens": 84000, "context_window_size": 200000, "used_percentage": 42 } });
        let c = claude_context(&input).unwrap();
        assert_eq!((c.used_percent, c.tokens, c.window), (42.0, 84000, 200000));
        assert!(claude_context(&json!({})).is_none());

        let log = r#"{"timestamp":"2026-10-14T00:10:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":47500},"model_context_window":475000}}}"#;
        let c = codex_context_in(log).unwrap();
        assert_eq!((c.used_percent, c.tokens), (10.0, 47500));
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

    #[test]
    fn reads_only_the_main_codex_bucket() {
        let log = [
            r#"{"timestamp":"2026-10-14T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":null,"secondary":{"used_percent":8.0,"window_minutes":10080,"resets_at":1792515988},"individual_limit":{"limit":"100","used":"0","remaining_percent":100,"resets_at":1793000000}}}}"#,
            r#"{"timestamp":"2026-10-14T00:10:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex_other","primary":null,"secondary":{"used_percent":23.0,"window_minutes":10080,"resets_at":1792515988}}}}"#,
        ]
        .join("\n");
        let s = codex_snapshot_in(&log).unwrap();
        assert_eq!(s.windows.len(), 2);
        assert_eq!((s.windows[0].kind.as_str(), s.windows[0].used_percent), ("week", 8.0));
        assert_eq!((s.windows[1].kind.as_str(), s.windows[1].used_percent, s.windows[1].resets_at), ("month-credit", 0.0, Some(1793000000000)));
    }

    #[test]
    fn reads_the_codex_usage_endpoint() {
        let body = json!({
            "plan_type": "business",
            "rate_limit": {
                "primary_window": { "used_percent": 42, "limit_window_seconds": 18000, "reset_after_seconds": 600, "reset_at": 1792073479 },
                "secondary_window": { "used_percent": 7, "limit_window_seconds": 604800, "reset_after_seconds": 9000 }
            },
            "spend_control": { "reached": false, "individual_limit": { "remaining_percent": 75, "resets_at": 1793000000 } }
        });
        let s = codex_usage_snapshot(&body, 1_000_000).unwrap();
        assert_eq!(s.windows[0], Window { kind: "session".into(), label: "5-hour".into(), used_percent: 42.0, resets_at: Some(1792073479000) });
        assert_eq!((s.windows[1].kind.as_str(), s.windows[1].used_percent, s.windows[1].resets_at), ("week", 7.0, Some(1_000_000 + 9_000_000)));
        assert_eq!((s.windows[2].kind.as_str(), s.windows[2].used_percent), ("month-credit", 25.0));
        assert!(codex_usage_snapshot(&json!({ "rate_limit": null }), 0).is_none());
    }

    /// Asks the real endpoint with this machine's Codex sign-in: `cargo test codex_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn codex_live_on_this_machine() {
        println!("{:#?}", codex_live());
    }

    #[test]
    fn reads_opencode_context() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"create table kv (key text primary key, value text);
            create table session_v2 (id text, parent_id text, directory text, model text, time_updated integer);
            create table session_message (session_id text, type text, seq integer, time_updated integer, data text);
            insert into kv values ('models-dev:catalog', '{"updatedAt":1,"body":"{\"opencode\":{\"models\":{\"big\":{\"limit\":{\"context\":200000}}}}}"}');
            insert into session_v2 values ('s1', null, '/w/a', '{"id":"big","providerID":"opencode"}', 2);
            insert into session_v2 values ('s0', null, '/w/b', null, 3);
            insert into session_message values ('s1', 'assistant', 1, 10, '{"tokens":{"input":100,"output":0,"cache":{"read":0,"write":0}}}');
            insert into session_message values ('s1', 'assistant', 2, 20, '{"model":{"id":"big","providerID":"opencode"},"tokens":{"input":1000,"output":500,"reasoning":500,"cache":{"read":48000,"write":0}}}');"#,
        )
        .unwrap();
        assert_eq!(opencode_model_at(&conn, "/w/a").unwrap()["id"], "big");
        assert!(opencode_model_at(&conn, "/w/c").is_none());
        let c = opencode_context_at(&conn, "/w/a/").unwrap();
        assert_eq!((c.tokens, c.window, c.used_percent, c.at), (50000, 200000, 25.0, 20));
        assert!(opencode_context_at(&conn, "/w/c").is_none());
    }

    #[test]
    fn tells_plan_sign_ins_from_api_keys() {
        let auth = json!({ "openai": { "type": "oauth", "access": "a", "refresh": "r", "expires": 1 }, "anthropic": { "type": "api", "key": "k" } });
        assert!(opencode_signed_in(&auth, "openai"));
        assert!(!opencode_signed_in(&auth, "anthropic"));
        assert!(!opencode_signed_in(&auth, "opencode"));
    }

    fn opendb_context(dir: &str) -> Option<Context> {
        opencode_context_at(&opencode_db()?, dir)
    }

    /// Reads this machine's real Codex logs: `cargo test codex_logs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn codex_logs_on_this_machine() {
        let root = codex_sessions().unwrap();
        let snapshot = newest_logs(&root, 4).iter().find_map(|log| codex_snapshot_in(&tail(log, 512 * 1024)?));
        println!("{snapshot:#?}");
        let log = codex_log_for(&root, None, "/Users/rickyperlick/Coding");
        let dir = "/Users/rickyperlick/Library/Application Support/dev.winfer.app/worktrees/cleadger-8999bec3/opencode-1";
        println!("opencode: {:#?}", opendb_context(dir));
        println!("{log:?} {:#?}", log.as_ref().and_then(|l| codex_context_in(&tail(l, 1024 * 1024)?)));
    }
}
