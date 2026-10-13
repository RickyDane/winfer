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
];

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
            "SELECT id, provider, title, cwd, worktree, race_id, model FROM panes WHERE workspace_id = ?1 ORDER BY sort",
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
                    "INSERT INTO panes (id, workspace_id, provider, title, cwd, worktree, sort, race_id, model)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
}
