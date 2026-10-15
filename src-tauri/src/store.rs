//! Persistent app state in SQLite: workspaces, panes and a few UI settings.
//!
//! The frontend owns the live state and saves a full snapshot (debounced); the data is small,
//! so each save rewrites it inside one transaction.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneRecord {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub cwd: String,
    /// `{ path, branch, base, repo }` when the pane runs in its own git worktree.
    pub worktree: Option<Value>,
    /// Groups panes started together by race mode.
    #[serde(default)]
    pub race_id: Option<String>,
    /// Model the agent was launched with.
    #[serde(default)]
    pub model: Option<String>,
    /// Title the agent gave its terminal.
    #[serde(default)]
    pub agent_title: Option<String>,
    /// Name the user gave the session; shown instead of the agent's title.
    #[serde(default)]
    pub custom_title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    pub root_dir: String,
    pub layout: Option<Value>,
    pub zoomed_pane_id: Option<String>,
    pub preset_index: i64,
    pub use_worktrees: bool,
    pub panes: Vec<PaneRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub workspaces: Vec<WorkspaceRecord>,
    /// Free-form UI settings (active workspace, sidebar/browser visibility, …).
    pub ui: Value,
}

const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        root_dir TEXT NOT NULL,
        sort INTEGER NOT NULL,
        layout TEXT,
        zoomed_pane_id TEXT,
        preset_index INTEGER NOT NULL DEFAULT 0,
        use_worktrees INTEGER NOT NULL DEFAULT 1
    );
    CREATE TABLE panes (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        provider TEXT NOT NULL,
        title TEXT NOT NULL,
        cwd TEXT NOT NULL,
        worktree TEXT,
        sort INTEGER NOT NULL
    );
    CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#,
    r#"
    ALTER TABLE panes ADD COLUMN race_id TEXT;
"#,
    r#"
    ALTER TABLE panes ADD COLUMN model TEXT;
"#,
    r#"
    ALTER TABLE panes ADD COLUMN agent_title TEXT;
    CREATE TABLE merges (
        id TEXT PRIMARY KEY,
        merged_at INTEGER NOT NULL,
        workspace_id TEXT,
        workspace TEXT NOT NULL,
        repo TEXT NOT NULL,
        branch TEXT NOT NULL,
        base TEXT NOT NULL,
        commit_sha TEXT,
        provider TEXT NOT NULL,
        title TEXT NOT NULL,
        summary TEXT,
        task TEXT,
        files TEXT NOT NULL,
        adds INTEGER NOT NULL DEFAULT 0,
        dels INTEGER NOT NULL DEFAULT 0,
        tests TEXT
    );
    CREATE INDEX merges_by_time ON merges (merged_at DESC);
"#,
    r#"
    ALTER TABLE panes ADD COLUMN custom_title TEXT;
"#,
    r#"
    ALTER TABLE merges ADD COLUMN cost REAL;
    ALTER TABLE merges ADD COLUMN tokens INTEGER;
"#,
];

/// A merged feature in the history (from the review inbox, the pane menu, voice or a race).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MergeRecord {
    pub id: String,
    /// Milliseconds since the epoch.
    pub merged_at: i64,
    pub workspace_id: Option<String>,
    pub workspace: String,
    pub repo: String,
    pub branch: String,
    pub base: String,
    /// The merge commit on `base` (filled in when recorded).
    #[serde(default)]
    pub commit_sha: Option<String>,
    pub provider: String,
    pub title: String,
    pub summary: Option<String>,
    pub task: Option<String>,
    pub files: Vec<String>,
    pub adds: i64,
    pub dels: i64,
    /// e.g. "pnpm test passed".
    pub tests: Option<String>,
    /// What the agents spent on it at API prices (US dollars), if known.
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub tokens: Option<i64>,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate(&conn).unwrap();
        Self {
            conn: Mutex::new(conn),
        }
    }

    pub fn load(&self) -> rusqlite::Result<Snapshot> {
        let conn = self.conn.lock().unwrap();
        let json = |s: Option<String>| s.and_then(|s| serde_json::from_str(&s).ok());

        let mut workspaces = conn
            .prepare(
                "SELECT id, name, root_dir, layout, zoomed_pane_id, preset_index, use_worktrees
                 FROM workspaces ORDER BY sort",
            )?
            .query_map([], |r| {
                Ok(WorkspaceRecord {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    root_dir: r.get(2)?,
                    layout: json(r.get(3)?),
                    zoomed_pane_id: r.get(4)?,
                    preset_index: r.get(5)?,
                    use_worktrees: r.get(6)?,
                    panes: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut panes = conn.prepare(
            "SELECT id, provider, title, cwd, worktree, race_id, model, agent_title, custom_title FROM panes WHERE workspace_id = ?1 ORDER BY sort",
        )?;
        for ws in &mut workspaces {
            ws.panes = panes
                .query_map([&ws.id], |r| {
                    Ok(PaneRecord {
                        id: r.get(0)?,
                        provider: r.get(1)?,
                        title: r.get(2)?,
                        cwd: r.get(3)?,
                        worktree: json(r.get(4)?),
                        race_id: r.get(5)?,
                        model: r.get(6)?,
                        agent_title: r.get(7)?,
                        custom_title: r.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }

        let ui = conn
            .query_row("SELECT value FROM kv WHERE key = 'ui'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);

        Ok(Snapshot { workspaces, ui })
    }

    pub fn save(&self, snapshot: &Snapshot) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM panes", [])?;
        tx.execute("DELETE FROM workspaces", [])?;
        for (i, ws) in snapshot.workspaces.iter().enumerate() {
            tx.execute(
                "INSERT INTO workspaces (id, name, root_dir, sort, layout, zoomed_pane_id, preset_index, use_worktrees)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ws.id,
                    ws.name,
                    ws.root_dir,
                    i as i64,
                    ws.layout.as_ref().map(Value::to_string),
                    ws.zoomed_pane_id,
                    ws.preset_index,
                    ws.use_worktrees,
                ],
            )?;
            for (j, pane) in ws.panes.iter().enumerate() {
                tx.execute(
                    "INSERT INTO panes (id, workspace_id, provider, title, cwd, worktree, sort, race_id, model, agent_title, custom_title)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        pane.id,
                        ws.id,
                        pane.provider,
                        pane.title,
                        pane.cwd,
                        pane.worktree.as_ref().map(Value::to_string),
                        j as i64,
                        pane.race_id,
                        pane.model,
                        pane.agent_title,
                        pane.custom_title,
                    ],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO kv (key, value) VALUES ('ui', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [snapshot.ui.to_string()],
        )?;
        tx.commit()
    }
}

impl Store {
    pub fn add_merge(&self, m: &MergeRecord) -> rusqlite::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO merges (id, merged_at, workspace_id, workspace, repo, branch, base, commit_sha,
                provider, title, summary, task, files, adds, dels, tests, cost, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                m.id, m.merged_at, m.workspace_id, m.workspace, m.repo, m.branch, m.base, m.commit_sha,
                m.provider, m.title, m.summary, m.task, serde_json::to_string(&m.files).unwrap_or_default(),
                m.adds, m.dels, m.tests, m.cost, m.tokens,
            ],
        )?;
        Ok(())
    }

    /// Newest first. Every word of `query` must appear (case-insensitive) in the title, summary,
    /// task, branch, workspace, provider, commit or file names.
    pub fn search_merges(&self, query: &str, workspace_id: Option<&str>, limit: i64) -> rusqlite::Result<Vec<MergeRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, merged_at, workspace_id, workspace, repo, branch, base, commit_sha, provider, title, summary,
                    task, files, adds, dels, tests, cost, tokens FROM merges WHERE 1 = 1",
        );
        let mut args: Vec<String> = Vec::new();
        if let Some(ws) = workspace_id {
            args.push(ws.to_string());
            sql.push_str(&format!(" AND workspace_id = ?{}", args.len()));
        }
        for word in query.split_whitespace().take(8) {
            // LIKE wildcards in the search text are matched literally.
            let escaped = word.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            args.push(format!("%{escaped}%"));
            let n = args.len();
            sql.push_str(&format!(
                " AND (title LIKE ?{n} ESCAPE '\\' OR summary LIKE ?{n} ESCAPE '\\' OR task LIKE ?{n} ESCAPE '\\'
                   OR branch LIKE ?{n} ESCAPE '\\' OR workspace LIKE ?{n} ESCAPE '\\' OR provider LIKE ?{n} ESCAPE '\\'
                   OR commit_sha LIKE ?{n} ESCAPE '\\' OR files LIKE ?{n} ESCAPE '\\')"
            ));
        }
        sql.push_str(&format!(" ORDER BY merged_at DESC LIMIT {}", limit.clamp(1, 500)));
        let rows = conn
            .prepare(&sql)?
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(MergeRecord {
                    id: r.get(0)?,
                    merged_at: r.get(1)?,
                    workspace_id: r.get(2)?,
                    workspace: r.get(3)?,
                    repo: r.get(4)?,
                    branch: r.get(5)?,
                    base: r.get(6)?,
                    commit_sha: r.get(7)?,
                    provider: r.get(8)?,
                    title: r.get(9)?,
                    summary: r.get(10)?,
                    task: r.get(11)?,
                    files: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
                    adds: r.get(13)?,
                    dels: r.get(14)?,
                    tests: r.get(15)?,
                    cost: r.get(16)?,
                    tokens: r.get(17)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn delete_merge(&self, id: &str) -> rusqlite::Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM merges WHERE id = ?1", [id])?;
        Ok(())
    }
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", i as i64 + 1)?;
    }
    Ok(())
}

#[tauri::command]
pub fn state_load(store: State<'_, Store>) -> Result<Snapshot, String> {
    store.load().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn state_save(store: State<'_, Store>, snapshot: Snapshot) -> Result<(), String> {
    store.save(&snapshot).map_err(|e| e.to_string())
}

/// Records a merge; the merge commit is read from the repo (it's HEAD of the base right after).
#[tauri::command]
pub fn history_add(store: State<'_, Store>, mut merge: MergeRecord) -> Result<(), String> {
    if merge.commit_sha.is_none() {
        merge.commit_sha = crate::env::no_window(&mut std::process::Command::new("git"))
            .args(["-C", &merge.repo, "rev-parse", "--short=10", "HEAD"])
            .envs(crate::env::login_env())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    }
    store.add_merge(&merge).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_search(store: State<'_, Store>, query: String, workspace_id: Option<String>) -> Result<Vec<MergeRecord>, String> {
    store.search_merges(&query, workspace_id.as_deref(), 200).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_delete(store: State<'_, Store>, id: String) -> Result<(), String> {
    store.delete_merge(&id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_then_load_round_trips() {
        let store = Store::in_memory();
        let snapshot: Snapshot = serde_json::from_value(json!({
            "workspaces": [
                {
                    "id": "ws-1", "name": "api", "rootDir": "~/api",
                    "layout": { "kind": "leaf", "paneId": "p-1" },
                    "zoomedPaneId": null, "presetIndex": 2, "useWorktrees": true,
                    "panes": [{ "id": "p-1", "provider": "claude", "title": "api", "cwd": "/tmp/wt",
                                "worktree": { "path": "/tmp/wt", "branch": "winfer/claude-1" }, "raceId": "race-1", "model": "opus" }]
                },
                { "id": "ws-2", "name": "home", "rootDir": "~", "layout": null, "zoomedPaneId": null,
                  "presetIndex": 0, "useWorktrees": false, "panes": [] }
            ],
            "ui": { "activeWorkspaceId": "ws-2" }
        }))
        .unwrap();

        store.save(&snapshot).unwrap();
        let loaded = serde_json::to_value(store.load().unwrap()).unwrap();
        assert_eq!(loaded, serde_json::to_value(&snapshot).unwrap());

        // A second save replaces, not appends.
        store.save(&snapshot).unwrap();
        assert_eq!(store.load().unwrap().workspaces.len(), 2);
    }

    #[test]
    fn keeps_agent_titles() {
        let store = Store::in_memory();
        let snapshot: Snapshot = serde_json::from_value(json!({
            "workspaces": [{ "id": "ws-1", "name": "api", "rootDir": "~", "layout": null, "zoomedPaneId": null,
                "presetIndex": 0, "useWorktrees": true,
                "panes": [{ "id": "p-1", "provider": "claude", "title": "api", "cwd": "~", "worktree": null,
                            "agentTitle": "Fix the login redirect", "customTitle": "Login bug" }] }],
            "ui": null
        }))
        .unwrap();
        store.save(&snapshot).unwrap();
        let pane = &store.load().unwrap().workspaces[0].panes[0];
        assert_eq!(pane.agent_title.as_deref(), Some("Fix the login redirect"));
        assert_eq!(pane.custom_title.as_deref(), Some("Login bug"));
    }

    #[test]
    fn searches_merges_by_every_word() {
        let store = Store::in_memory();
        let merge = |id: &str, at: i64, ws: &str, title: &str, files: &[&str]| MergeRecord {
            id: id.into(),
            merged_at: at,
            workspace_id: Some(ws.into()),
            workspace: ws.into(),
            repo: "/repo".into(),
            branch: format!("winfer/claude-{id}"),
            base: "main".into(),
            commit_sha: Some(format!("abc{id}")),
            provider: "claude".into(),
            title: title.into(),
            summary: Some("Adds a guard; 100% covered".into()),
            task: None,
            files: files.iter().map(|f| f.to_string()).collect(),
            adds: 10,
            dels: 2,
            tests: None,
            cost: Some(1.25),
            tokens: Some(420_000),
        };
        store.add_merge(&merge("1", 1, "api", "Fix the login redirect", &["src/auth/login.ts"])).unwrap();
        store.add_merge(&merge("2", 2, "api", "Rate limit billing webhooks", &["src/billing/hooks.ts"])).unwrap();
        store.add_merge(&merge("3", 3, "web", "Login page dark mode", &["app/login.tsx"])).unwrap();

        let ids = |q: &str, ws: Option<&str>| store.search_merges(q, ws, 50).unwrap().into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids("", None), ["3", "2", "1"], "newest first");
        assert_eq!(ids("login", None), ["3", "1"]);
        assert_eq!(ids("LOGIN redirect", None), ["1"], "every word must match");
        assert_eq!(ids("billing/hooks", None), ["2"], "file names are searched");
        assert_eq!(ids("login", Some("web")), ["3"], "workspace filter");
        assert_eq!(ids("100%", None).len(), 3, "% is literal");
        assert_eq!(ids("50%", None), Vec::<String>::new());
        store.delete_merge("2").unwrap();
        assert_eq!(ids("", None), ["3", "1"]);
    }
}
