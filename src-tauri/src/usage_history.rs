//! Token use over the last 90 days and what it would cost at API prices.
//!
//! The agents' own logs are the source: Claude Code's transcripts (`~/.claude/projects`),
//! Codex's session logs (`~/.codex/sessions`) and OpenCode's database. They're read
//! incrementally into `usage.db` (one row per model call), so the history survives the
//! agents cleaning up their logs (Claude Code deletes transcripts after 30 days). Rows older
//! than 90 days are dropped.
//!
//! Each call also keeps the folder the agent worked in, so spend can be told per task: an
//! agent's worktree is its task (see `dir_usage`).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

pub const KEEP_DAYS: i64 = 90;
const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;

pub struct UsageHistory {
    conn: Mutex<Connection>,
    /// When the logs were last read, so frequent lookups don't walk them every time.
    scanned: Mutex<Option<std::time::Instant>>,
}

/// One model call's tokens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Event {
    pub id: String,
    /// Unix ms.
    pub at: i64,
    pub provider: String,
    pub model: String,
    /// Input tokens not read from or written to the cache.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// The cost the agent reported itself (OpenCode), used over the price table.
    pub cost: Option<f64>,
    /// Folder the agent ran in (forward slashes, no trailing slash), if the log says.
    pub cwd: Option<String>,
}

/// Tokens and cost of one provider/model in one hour.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    /// Start of the hour (unix ms).
    pub hour: i64,
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    /// False when the model has no known price (its cost counts as 0).
    pub priced: bool,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl UsageHistory {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                at INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                input INTEGER NOT NULL,
                output INTEGER NOT NULL,
                cache_read INTEGER NOT NULL,
                cache_write INTEGER NOT NULL,
                cost REAL
            );
            CREATE INDEX IF NOT EXISTS events_by_time ON events (at);
            -- How far each log has been read, and what's needed to carry on from there.
            CREATE TABLE IF NOT EXISTS files (path TEXT PRIMARY KEY, offset INTEGER NOT NULL, state TEXT);
            CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        if conn.prepare("SELECT cwd FROM events LIMIT 0").is_err() {
            // Added later: read every log again so the rows still there get their folder.
            conn.execute_batch(
                "ALTER TABLE events ADD COLUMN cwd TEXT;
                 DELETE FROM files;
                 DELETE FROM kv WHERE key = 'opencode';",
            )?;
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS events_by_cwd ON events (cwd, at);")?;
        Ok(Self { conn: Mutex::new(conn), scanned: Mutex::new(None) })
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn insert(conn: &Connection, events: &[Event]) -> rusqlite::Result<()> {
        let mut stmt = conn.prepare_cached(
            "INSERT OR REPLACE INTO events (id, at, provider, model, input, output, cache_read, cache_write, cost, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for e in events {
            stmt.execute(params![e.id, e.at, e.provider, e.model, e.input as i64, e.output as i64, e.cache_read as i64, e.cache_write as i64, e.cost, e.cwd])?;
        }
        Ok(())
    }

    /// Reads what's new in the agents' logs and drops rows past the 90 days.
    pub fn scan(&self) {
        let cutoff = now_ms() - KEEP_DAYS * DAY_MS;
        let mut conn = self.conn.lock().unwrap();
        if let Some(root) = claude_projects() {
            for file in jsonl_files(&root, cutoff) {
                let _ = scan_file(&mut conn, &file, cutoff, claude_events);
            }
        }
        if let Some(root) = codex_sessions() {
            for file in jsonl_files(&root, cutoff) {
                let _ = scan_file(&mut conn, &file, cutoff, codex_events);
            }
        }
        let _ = scan_opencode(&mut conn, cutoff);
        let _ = conn.execute("DELETE FROM events WHERE at < ?1", [cutoff]);
        *self.scanned.lock().unwrap() = Some(std::time::Instant::now());
    }

    /// Scans unless that happened less than `max_age` ago.
    pub fn scan_if_stale(&self, max_age: std::time::Duration) {
        let fresh = self.scanned.lock().unwrap().is_some_and(|t| t.elapsed() < max_age);
        if !fresh {
            self.scan();
        }
    }

    /// Spend in `dir` (and folders under it) since `since` (unix ms).
    pub fn dir_usage(&self, dir: &str, since: i64, prices: &Prices) -> rusqlite::Result<DirUsage> {
        let dir = norm_dir(dir);
        let under = format!("{}/%", dir.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT ?1, provider, model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write), SUM(cost), COUNT(cost),
                    MIN(at), MAX(at), COUNT(*)
             FROM events WHERE (cwd = ?1 OR cwd LIKE ?2 ESCAPE '\\') AND at >= ?3 GROUP BY provider, model",
        )?;
        let rows = stmt.query_map(params![dir, under, since], dir_row)?;
        let mut out = fold_dirs(rows, prices)?;
        Ok(out.pop().unwrap_or_else(|| DirUsage { dir, ..Default::default() }))
    }

    /// Spend per checkout (repo or worktree) since `since`, most expensive first, for telling
    /// which tasks cost the most.
    pub fn by_dir(&self, since: i64, prices: &Prices) -> rusqlite::Result<Vec<DirUsage>> {
        self.by_dir_with(since, prices, checkout_root)
    }

    fn by_dir_with(&self, since: i64, prices: &Prices, root: impl Fn(&str) -> String) -> rusqlite::Result<Vec<DirUsage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT cwd, provider, model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write), SUM(cost), COUNT(cost),
                    MIN(at), MAX(at), COUNT(*)
             FROM events WHERE cwd IS NOT NULL AND at >= ?1 GROUP BY cwd, provider, model",
        )?;
        let rows = stmt.query_map(params![since], dir_row)?;
        Ok(group_by_checkout(fold_dirs(rows, prices)?, root))
    }

    /// Hourly totals per model since `since` (unix ms).
    pub fn buckets(&self, since: i64, prices: &Prices) -> rusqlite::Result<Vec<Bucket>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT (at / ?2) * ?2 AS hour, provider, model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write),
                    SUM(cost), COUNT(cost)
             FROM events WHERE at >= ?1 GROUP BY hour, provider, model ORDER BY hour",
        )?;
        let rows = stmt.query_map(params![since, HOUR_MS], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                [r.get::<_, i64>(3)? as u64, r.get::<_, i64>(4)? as u64, r.get::<_, i64>(5)? as u64, r.get::<_, i64>(6)? as u64],
                r.get::<_, Option<f64>>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (hour, provider, model, [input, output, cache_read, cache_write], reported, reported_rows) = row?;
            let (cost, priced) = cost_of(prices, &model, reported_rows > 0, reported, [input, output, cache_read, cache_write]);
            out.push(Bucket { hour, provider, model, input, output, cache_read, cache_write, cost, priced });
        }
        Ok(out)
    }
}

/// Tokens and cost of one model within a folder's spend.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub priced: bool,
}

/// What the agents spent working in one folder (an agent's worktree is its task).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirUsage {
    pub dir: String,
    pub cost: f64,
    /// False when a model has no known price (its cost counts as 0).
    pub priced: bool,
    pub tokens: u64,
    /// Model calls.
    pub calls: u64,
    /// First and last call (unix ms).
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Most expensive first.
    pub models: Vec<ModelUsage>,
}

impl Default for DirUsage {
    fn default() -> Self {
        Self { dir: String::new(), cost: 0.0, priced: true, tokens: 0, calls: 0, first_at: None, last_at: None, models: Vec::new() }
    }
}

type DirRow = (String, String, String, [u64; 4], Option<f64>, i64, [i64; 3]);

/// dir, provider, model, token sums, reported cost sum and count, first, last, calls.
fn dir_row(r: &rusqlite::Row) -> rusqlite::Result<DirRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        [r.get::<_, i64>(3)? as u64, r.get::<_, i64>(4)? as u64, r.get::<_, i64>(5)? as u64, r.get::<_, i64>(6)? as u64],
        r.get(7)?,
        r.get(8)?,
        [r.get(9)?, r.get(10)?, r.get(11)?],
    ))
}

/// Rows per folder and model into one total per folder, most expensive first.
fn fold_dirs(rows: impl Iterator<Item = rusqlite::Result<DirRow>>, prices: &Prices) -> rusqlite::Result<Vec<DirUsage>> {
    let mut by: HashMap<String, DirUsage> = HashMap::new();
    for row in rows {
        let (dir, provider, model, [input, output, cache_read, cache_write], reported, reported_rows, [first, last, calls]) = row?;
        let (cost, priced) = cost_of(prices, &model, reported_rows > 0, reported, [input, output, cache_read, cache_write]);
        let d = by.entry(dir.clone()).or_insert_with(|| DirUsage { dir, ..Default::default() });
        d.cost += cost;
        d.priced &= priced;
        d.tokens += input + output + cache_read + cache_write;
        d.calls += calls as u64;
        d.first_at = Some(d.first_at.map_or(first, |f| f.min(first)));
        d.last_at = Some(d.last_at.map_or(last, |l| l.max(last)));
        d.models.push(ModelUsage { provider, model, input, output, cache_read, cache_write, cost, priced });
    }
    let mut out: Vec<DirUsage> = by.into_values().collect();
    for d in &mut out {
        d.models.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    }
    out.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(b.tokens.cmp(&a.tokens)));
    Ok(out)
}

/// The checkout a folder belongs to (the nearest folder above it with a `.git`, so a repo or a
/// worktree), or the folder itself when there's none (or it's gone).
fn checkout_root(dir: &str) -> String {
    let mut path = Some(Path::new(dir));
    while let Some(p) = path {
        if p.join(".git").exists() {
            return norm_dir(&p.to_string_lossy());
        }
        path = p.parent();
    }
    dir.to_string()
}

/// Merges folders of the same checkout: an agent that `cd`s into `src` is still on the same task.
fn group_by_checkout(dirs: Vec<DirUsage>, root: impl Fn(&str) -> String) -> Vec<DirUsage> {
    let mut by: HashMap<String, DirUsage> = HashMap::new();
    for d in dirs {
        let key = root(&d.dir);
        let Some(p) = by.get_mut(&key) else {
            by.insert(key.clone(), DirUsage { dir: key, ..d });
            continue;
        };
        p.cost += d.cost;
        p.priced &= d.priced;
        p.tokens += d.tokens;
        p.calls += d.calls;
        p.first_at = [p.first_at, d.first_at].into_iter().flatten().min();
        p.last_at = p.last_at.max(d.last_at);
        for m in d.models {
            match p.models.iter_mut().find(|x| x.provider == m.provider && x.model == m.model) {
                Some(x) => {
                    x.input += m.input;
                    x.output += m.output;
                    x.cache_read += m.cache_read;
                    x.cache_write += m.cache_write;
                    x.cost += m.cost;
                }
                None => p.models.push(m),
            }
        }
    }
    let mut out: Vec<DirUsage> = by.into_values().collect();
    for d in &mut out {
        d.models.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    }
    out.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(b.tokens.cmp(&a.tokens)));
    out
}

/// OpenCode reports a cost per call; elsewhere it comes from the price table.
fn cost_of(prices: &Prices, model: &str, reported: bool, reported_cost: Option<f64>, [input, output, cache_read, cache_write]: [u64; 4]) -> (f64, bool) {
    match (reported, prices.get(model)) {
        (true, _) => (reported_cost.unwrap_or(0.0), true),
        (false, Some(p)) => (p.cost(input, output, cache_read, cache_write), true),
        (false, None) => (0.0, false),
    }
}

/// Folders compare with forward slashes and without a trailing slash.
pub fn norm_dir(dir: &str) -> String {
    let d = dir.replace('\\', "/");
    match d.trim_end_matches('/') {
        "" => d,
        trimmed => trimmed.to_string(),
    }
}

/// Reads the complete lines added to `path` since the last scan.
fn scan_file(conn: &mut Connection, path: &Path, cutoff: i64, parse: fn(&str, &Path, u64, &mut Value) -> Option<Event>) -> rusqlite::Result<()> {
    let key = path.to_string_lossy().to_string();
    let (mut offset, state): (u64, Option<String>) = conn
        .query_row("SELECT offset, state FROM files WHERE path = ?1", [&key], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))
        .optional()?
        .unwrap_or((0, None));
    let mut state: Value = state.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let Ok(mut file) = std::fs::File::open(path) else { return Ok(()) };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len < offset {
        // Rewritten: start over.
        offset = 0;
        state = Value::Null;
    }
    if len == offset {
        return Ok(());
    }
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Ok(());
    }
    let mut buf = Vec::new();
    let _ = file.take(len - offset).read_to_end(&mut buf);
    // A line still being written is read next time.
    let Some(end) = buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1) else { return Ok(()) };

    let mut events = Vec::new();
    let mut pos = offset;
    for line in buf[..end].split(|b| *b == b'\n') {
        let at = pos;
        pos += line.len() as u64 + 1;
        let Ok(line) = std::str::from_utf8(line) else { continue };
        if let Some(e) = parse(line, path, at, &mut state).filter(|e| e.at >= cutoff) {
            events.push(e);
        }
    }
    let tx = conn.transaction()?;
    UsageHistory::insert(&tx, &events)?;
    tx.execute(
        "INSERT OR REPLACE INTO files (path, offset, state) VALUES (?1, ?2, ?3)",
        params![key, (offset + end as u64) as i64, (!state.is_null()).then(|| state.to_string())],
    )?;
    tx.commit()
}

/// `.jsonl` files under `root` (any depth) changed since `since`.
fn jsonl_files(root: &Path, since: i64) -> Vec<PathBuf> {
    let since = std::time::UNIX_EPOCH + std::time::Duration::from_millis(since.max(0) as u64);
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && entry.metadata().ok().and_then(|m| m.modified().ok()).is_some_and(|m| m >= since)
            {
                out.push(path);
            }
        }
    }
    out
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn timestamp(v: &Value) -> Option<i64> {
    v["timestamp"].as_str().and_then(crate::usage::parse_rfc3339)
}

// Claude Code ------------------------------------------------------------------------------------

fn claude_projects() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    Some(dir.join("projects"))
}

/// An assistant message in a transcript. A response split into several lines repeats the
/// message id; the row is replaced, so the last line's (final) usage counts once.
fn claude_events(line: &str, _path: &Path, _at: u64, _state: &mut Value) -> Option<Event> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] != "assistant" {
        return None;
    }
    let m = &v["message"];
    let u = &m["usage"];
    let model = m["model"].as_str()?;
    if model == "<synthetic>" {
        return None;
    }
    let id = m["id"].as_str().or_else(|| v["uuid"].as_str())?;
    let request = v["requestId"].as_str().unwrap_or("");
    Some(Event {
        id: format!("claude:{id}:{request}"),
        at: timestamp(&v)?,
        provider: "claude".into(),
        model: model.into(),
        input: n(&u["input_tokens"]),
        output: n(&u["output_tokens"]),
        cache_read: n(&u["cache_read_input_tokens"]),
        cache_write: n(&u["cache_creation_input_tokens"]),
        cost: None,
        cwd: v["cwd"].as_str().map(norm_dir),
    })
    .filter(|e| e.input + e.output + e.cache_read + e.cache_write > 0)
}

// Codex -----------------------------------------------------------------------------------------

fn codex_sessions() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
    Some(home.join("sessions"))
}

/// A `token_count` event after a turn. Codex repeats the event without new usage (e.g. for
/// rate limit updates), so a turn counts only when the session's running total grew. The
/// model comes from the latest `turn_context`. `state` carries both across scans.
fn codex_events(line: &str, path: &Path, at: u64, state: &mut Value) -> Option<Event> {
    if line.contains("\"turn_context\"") || line.contains("\"session_meta\"") {
        let v: Value = serde_json::from_str(line).ok()?;
        if let Some(model) = v["payload"]["model"].as_str() {
            state["model"] = model.into();
        }
        if let Some(cwd) = v["payload"]["cwd"].as_str() {
            state["cwd"] = norm_dir(cwd).into();
        }
        return None;
    }
    if !line.contains("\"token_count\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let info = &v["payload"]["info"];
    let total = info["total_token_usage"]["total_tokens"].as_u64()?;
    if state["total"].as_u64().is_some_and(|t| t >= total) {
        return None;
    }
    state["total"] = total.into();
    let u = &info["last_token_usage"];
    // OpenAI counts cached input inside `input_tokens`.
    let cache_read = n(&u["cached_input_tokens"]);
    let cache_write = n(&u["cache_write_input_tokens"]);
    Some(Event {
        id: format!("codex:{}:{at}", path.file_name()?.to_string_lossy()),
        at: timestamp(&v)?,
        provider: "codex".into(),
        model: state["model"].as_str().unwrap_or("unknown").into(),
        input: n(&u["input_tokens"]).saturating_sub(cache_read + cache_write),
        // Reasoning tokens are already part of the output.
        output: n(&u["output_tokens"]),
        cache_read,
        cache_write,
        cost: None,
        cwd: state["cwd"].as_str().map(String::from),
    })
}

// OpenCode --------------------------------------------------------------------------------------

/// Assistant messages updated since the last scan (with a day's margin for ones still running).
fn scan_opencode(conn: &mut Connection, cutoff: i64) -> rusqlite::Result<()> {
    let Some(db) = crate::usage::opencode_db() else { return Ok(()) };
    let mark: i64 = conn
        .query_row("SELECT value FROM kv WHERE key = 'opencode'", [], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let since = cutoff.max(mark - DAY_MS);
    // The folder is the session's; databases without the sessions table go without.
    let mut stmt = db
        .prepare(
            "SELECT m.id, m.time_created, m.time_updated, m.data, s.directory FROM session_message m
             LEFT JOIN session_v2 s ON s.id = m.session_id WHERE m.type = 'assistant' AND m.time_updated >= ?1",
        )
        .or_else(|_| db.prepare("SELECT id, time_created, time_updated, data, NULL FROM session_message WHERE type = 'assistant' AND time_updated >= ?1"))?;
    let mut newest = mark;
    let events: Vec<Event> = stmt
        .query_map([since], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?))
        })?
        .flatten()
        .filter_map(|(id, created, updated, data, dir)| {
            newest = newest.max(updated);
            let mut e = opencode_event(&id, created, &serde_json::from_str(&data).ok()?)?;
            e.cwd = dir.as_deref().map(norm_dir);
            Some(e)
        })
        .filter(|e| e.at >= cutoff)
        .collect();
    let tx = conn.transaction()?;
    UsageHistory::insert(&tx, &events)?;
    tx.execute("INSERT OR REPLACE INTO kv (key, value) VALUES ('opencode', ?1)", [newest.to_string()])?;
    tx.commit()
}

fn opencode_event(id: &str, at: i64, data: &Value) -> Option<Event> {
    let t = &data["tokens"];
    let model = data["model"]["id"].as_str().or_else(|| data["modelID"].as_str())?;
    let e = Event {
        id: format!("opencode:{id}"),
        at,
        provider: "opencode".into(),
        model: model.into(),
        input: n(&t["input"]),
        output: n(&t["output"]) + n(&t["reasoning"]),
        cache_read: n(&t["cache"]["read"]),
        cache_write: n(&t["cache"]["write"]),
        cost: data["cost"].as_f64().filter(|c| *c > 0.0),
        cwd: None,
    };
    (e.input + e.output + e.cache_read + e.cache_write > 0).then_some(e)
}

// Prices ----------------------------------------------------------------------------------------

/// US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Price {
    const fn new(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Self {
        Self { input, output, cache_read, cache_write }
    }

    pub fn cost(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
        (input as f64 * self.input + output as f64 * self.output + cache_read as f64 * self.cache_read + cache_write as f64 * self.cache_write) / 1e6
    }
}

/// Standard API prices (base context tier). Longest matching prefix wins.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", Price::new(10.0, 50.0, 0.25, 12.5)),
    ("claude-fable-5", Price::new(10.0, 50.0, 1.0, 12.5)),
    ("claude-opus-5", Price::new(4.0, 20.0, 0.2, 5.0)),
    ("claude-sonnet-5", Price::new(2.0, 10.0, 0.1, 2.5)),
    ("claude-haiku-5", Price::new(0.1, 0.5, 0.01, 0.125)),
    ("claude-opus-4-1", Price::new(15.0, 75.0, 1.5, 18.75)),
    ("claude-opus-4-0", Price::new(15.0, 75.0, 1.5, 18.75)),
    ("claude-opus-4-2025", Price::new(15.0, 75.0, 1.5, 18.75)),
    ("claude-opus-4", Price::new(5.0, 25.0, 0.5, 6.25)),
    ("claude-sonnet-4", Price::new(3.0, 15.0, 0.3, 3.75)),
    ("claude-3-7-sonnet", Price::new(3.0, 15.0, 0.3, 3.75)),
    ("claude-haiku-4", Price::new(1.0, 5.0, 0.1, 1.25)),
    ("claude-3-5-haiku", Price::new(0.8, 4.0, 0.08, 1.0)),
    ("gpt-6.1-sol", Price::new(2.0, 10.0, 0.1, 2.5)),
    ("gpt-6-sol", Price::new(2.0, 10.0, 0.2, 2.5)),
    ("gpt-6-luna", Price::new(0.1, 0.5, 0.01, 0.125)),
    ("gpt-6-astra", Price::new(10.0, 50.0, 1.0, 12.5)),
    ("gpt-5.6-sol", Price::new(4.0, 20.0, 0.4, 5.0)),
    ("gpt-5.6-terra", Price::new(2.0, 12.0, 0.2, 2.5)),
    ("gpt-5.6-luna", Price::new(0.2, 1.2, 0.02, 0.25)),
    ("gpt-5.6", Price::new(4.0, 20.0, 0.4, 5.0)),
    ("gpt-5.5-pro", Price::new(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5.5", Price::new(5.0, 30.0, 0.5, 5.0)),
    ("gpt-5.4-pro", Price::new(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5.4-mini", Price::new(0.75, 4.5, 0.075, 0.75)),
    ("gpt-5.4-nano", Price::new(0.2, 1.25, 0.02, 0.2)),
    ("gpt-5.4", Price::new(2.5, 15.0, 0.25, 2.5)),
    ("gpt-5.3", Price::new(1.75, 14.0, 0.175, 1.75)),
    ("gpt-5.2-pro", Price::new(21.0, 168.0, 21.0, 21.0)),
    ("gpt-5.2", Price::new(1.75, 14.0, 0.175, 1.75)),
    ("gpt-5-pro", Price::new(15.0, 120.0, 15.0, 15.0)),
    ("gpt-5-mini", Price::new(0.25, 2.0, 0.025, 0.25)),
    ("gpt-5-nano", Price::new(0.05, 0.4, 0.005, 0.05)),
    ("gpt-5", Price::new(1.25, 10.0, 0.125, 1.25)),
];

/// The price table, plus the models.dev catalog OpenCode keeps (when it's installed) for
/// models the table doesn't know.
pub struct Prices {
    catalog: HashMap<String, Price>,
}

impl Prices {
    pub fn load() -> Self {
        Self { catalog: crate::usage::opencode_db().and_then(|db| models_dev_prices(&db)).unwrap_or_default() }
    }

    /// "anthropic/claude-sonnet-4-5-20250929", "claude-opus-5-5[1m]" → "claude-sonnet-4-5-20250929", "claude-opus-5-5".
    fn normalize(model: &str) -> String {
        let m = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
        let m = m.split('[').next().unwrap_or(&m).to_string();
        // Bedrock / Vertex ids: "us.anthropic.claude-…-v1:0", "claude-…@20250514".
        let m = m.rsplit("anthropic.").next().unwrap_or(&m).to_string();
        m.split(['@', ':']).next().unwrap_or(&m).to_string()
    }

    pub fn get(&self, model: &str) -> Option<Price> {
        let m = Self::normalize(model);
        PRICES
            .iter()
            .filter(|(prefix, _)| m.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, p)| *p)
            .or_else(|| self.catalog.get(&m).copied())
    }
}

fn models_dev_prices(db: &Connection) -> Option<HashMap<String, Price>> {
    let raw: String = db.query_row("SELECT value FROM kv WHERE key = 'models-dev:catalog'", [], |r| r.get(0)).ok()?;
    let outer: Value = serde_json::from_str(&raw).ok()?;
    let body: Value = match &outer["body"] {
        Value::String(s) => serde_json::from_str(s).ok()?,
        v => v.clone(),
    };
    let mut out = HashMap::new();
    for p in body.as_object()?.values() {
        for (id, m) in p["models"].as_object().into_iter().flatten() {
            let c = &m["cost"];
            let (Some(input), Some(output)) = (c["input"].as_f64(), c["output"].as_f64()) else { continue };
            out.entry(id.to_ascii_lowercase()).or_insert(Price {
                input,
                output,
                cache_read: c["cache_read"].as_f64().unwrap_or(input),
                cache_write: c["cache_write"].as_f64().unwrap_or(input),
            });
        }
    }
    Some(out)
}

// Commands ---------------------------------------------------------------------------------------

/// Rescans every half hour, so the history keeps what the agents later delete.
pub fn start(app: &tauri::AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            app.state::<UsageHistory>().scan();
            std::thread::sleep(std::time::Duration::from_secs(30 * 60));
        }
    });
}

#[tauri::command]
pub async fn usage_history(app: tauri::AppHandle, days: i64) -> Result<Vec<Bucket>, String> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        let history = app.state::<UsageHistory>();
        history.scan();
        let since = now_ms() - days.clamp(1, KEEP_DAYS) * DAY_MS;
        history.buckets(since, &Prices::load()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// When a folder was made (unix ms). A removed worktree's folder name is reused by a later task,
/// so a folder's spend only counts from then on.
fn created_at(dir: &str) -> Option<i64> {
    let t = std::fs::metadata(dir).ok()?.created().ok()?;
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// What the agents spent in each of `dirs` (worktrees, typically) since the folder was made.
#[tauri::command]
pub async fn usage_dirs(app: tauri::AppHandle, dirs: Vec<String>) -> Result<Vec<DirUsage>, String> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        let history = app.state::<UsageHistory>();
        history.scan_if_stale(std::time::Duration::from_secs(20));
        let prices = Prices::load();
        let floor = now_ms() - KEEP_DAYS * DAY_MS;
        dirs.iter()
            // A minute's margin: the agent can start before the folder's timestamp settles.
            .map(|d| history.dir_usage(d, created_at(d).map_or(floor, |c| (c - 60_000).max(floor)), &prices))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Spend per folder over the last `days`, most expensive first.
#[tauri::command]
pub async fn usage_by_dir(app: tauri::AppHandle, days: i64) -> Result<Vec<DirUsage>, String> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        let history = app.state::<UsageHistory>();
        history.scan_if_stale(std::time::Duration::from_secs(20));
        let since = now_ms() - days.clamp(1, KEEP_DAYS) * DAY_MS;
        history.by_dir(since, &Prices::load()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spend_per_folder_counts_subfolders_but_not_siblings() {
        let h = UsageHistory::in_memory();
        let ev = |id: &str, at: i64, cwd: Option<&str>, out: u64| Event {
            id: id.into(),
            at,
            provider: "claude".into(),
            model: "claude-opus-5-5".into(),
            output: out,
            cwd: cwd.map(norm_dir),
            ..Default::default()
        };
        UsageHistory::insert(
            &h.conn.lock().unwrap(),
            &[
                ev("a", 10, Some("/wt/claude-1"), 1_000_000),
                ev("b", 20, Some("/wt/claude-1/src/"), 1_000_000),
                ev("c", 30, Some("/wt/claude-10"), 1_000_000),
                ev("d", 5, Some("/wt/claude-1"), 1_000_000),
                ev("e", 40, None, 1_000_000),
            ],
        )
        .unwrap();
        let prices = Prices { catalog: HashMap::new() };
        let d = h.dir_usage("/wt/claude-1/", 10, &prices).unwrap();
        // "a" and "b" (subfolder); not the sibling "claude-10", nor "d" from before the folder was made.
        assert_eq!((d.calls, d.tokens, d.first_at, d.last_at), (2, 2_000_000, Some(10), Some(20)));
        assert!((d.cost - 40.0).abs() < 1e-9, "{}", d.cost);
        assert_eq!(h.dir_usage("/nowhere", 0, &prices).unwrap().calls, 0);

        // Per checkout: claude-1 (with its src), claude-10, and none for no folder.
        let as_checkout = |d: &str| if d.starts_with("/wt/claude-1/") { "/wt/claude-1".to_string() } else { d.to_string() };
        let all = h.by_dir_with(0, &prices, as_checkout).unwrap();
        let dirs: Vec<_> = all.iter().map(|d| (d.dir.as_str(), d.calls, d.models.len())).collect();
        assert_eq!(dirs, vec![("/wt/claude-1", 3, 1), ("/wt/claude-10", 1, 1)]);
        assert_eq!((all[0].first_at, all[0].last_at), (Some(5), Some(20)));
        assert_eq!(norm_dir(r"C:\wt\x\"), "C:/wt/x");
    }

    #[test]
    fn reads_claude_transcripts_once_per_message() {
        let dir = std::env::temp_dir().join(format!("winfer-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        let at = "2026-10-14T10:00:00.000Z";
        let line = |out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{at}","requestId":"r1","message":{{"id":"m1","model":"claude-opus-5-5","usage":{{"input_tokens":10,"output_tokens":{out},"cache_read_input_tokens":1000,"cache_creation_input_tokens":100}}}}}}"#
            )
        };
        std::fs::write(&file, format!("{}\n{}\n{{\"type\":\"user\"}}\n{}", line(5), line(50), line(7))).unwrap();

        let h = UsageHistory::in_memory();
        let mut conn = h.conn.lock().unwrap();
        scan_file(&mut conn, &file, 0, claude_events).unwrap();
        let rows: Vec<(i64, i64)> = conn.prepare("SELECT input, output FROM events").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect();
        // The unfinished last line isn't read yet.
        assert_eq!(rows, vec![(10, 50)]);
        drop(conn);

        let b = h.buckets(0, &Prices { catalog: HashMap::new() }).unwrap();
        assert_eq!(b.len(), 1);
        assert!((b[0].cost - (10.0 * 4.0 + 50.0 * 20.0 + 1000.0 * 0.2 + 100.0 * 5.0) / 1e6).abs() < 1e-12);
        assert_eq!(b[0].hour, crate::usage::parse_rfc3339(at).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_codex_turns_when_the_total_grows() {
        let mut state = Value::Null;
        let path = Path::new("rollout-x.jsonl");
        let ctx = r#"{"timestamp":"2026-10-14T10:00:00.000Z","type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#;
        let count = |total: u64| {
            format!(
                r#"{{"timestamp":"2026-10-14T10:01:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":50}}}}}}}}"#
            )
        };
        let meta = r#"{"timestamp":"2026-10-14T10:00:00.000Z","type":"session_meta","payload":{"cwd":"/w/codex-1/"}}"#;
        assert!(codex_events(meta, path, 0, &mut state).is_none());
        assert!(codex_events(ctx, path, 0, &mut state).is_none());
        let e = codex_events(&count(1050), path, 10, &mut state).unwrap();
        assert_eq!((e.model.as_str(), e.input, e.cache_read, e.output), ("gpt-6.1-sol", 200, 800, 50));
        assert_eq!(e.cwd.as_deref(), Some("/w/codex-1"));
        assert!(codex_events(&count(1050), path, 20, &mut state).is_none());
        assert!(codex_events(&count(2100), path, 30, &mut state).is_some());
    }

    #[test]
    fn prices_models() {
        let p = Prices { catalog: HashMap::new() };
        assert_eq!(p.get("claude-opus-5-5[1m]").unwrap().input, 4.0);
        assert_eq!(p.get("anthropic/claude-sonnet-4-5-20250929").unwrap().input, 3.0);
        assert_eq!(p.get("us.anthropic.claude-opus-4-1-20250805-v1:0").unwrap().input, 15.0);
        assert_eq!(p.get("claude-opus-4-6").unwrap().input, 5.0);
        assert_eq!(p.get("gpt-5.4-mini").unwrap().input, 0.75);
        assert!(p.get("llama-3").is_none());
    }
}

#[cfg(test)]
mod machine {
    /// Scans this machine's real logs: `cargo test usage_history_on_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn usage_history_on_this_machine() {
        use super::*;
        let path = std::env::temp_dir().join("winfer-usage-test.db");
        let _ = std::fs::remove_file(&path);
        let h = UsageHistory::open(&path).unwrap();
        let t = std::time::Instant::now();
        h.scan();
        println!("first scan {:?}", t.elapsed());
        let t = std::time::Instant::now();
        h.scan();
        println!("second scan {:?}", t.elapsed());
        let mut by_model: HashMap<String, (u64, f64, bool)> = HashMap::new();
        for b in h.buckets(0, &Prices::load()).unwrap() {
            let e = by_model.entry(format!("{}/{}", b.provider, b.model)).or_default();
            e.0 += b.input + b.output + b.cache_read + b.cache_write;
            e.1 += b.cost;
            e.2 = b.priced;
        }
        println!("{by_model:#?}");
        for d in h.by_dir(0, &Prices::load()).unwrap().iter().take(8) {
            println!("{:>9.2} $ {:>12} tok  {}", d.cost, d.tokens, d.dir);
        }
    }
}
