//! One SQLite database, one connection, one writer. Schema changes are numbered migrations.

mod blobs;
mod import;
mod mcp;
mod migrations;
mod reads;
mod sessions;
mod settings;
mod staged;
pub(crate) mod tasks;
mod todos;
mod tree;

pub use import::ImportCheckpoints;
pub use mcp::Renamed;
pub use sessions::{Admit, Admitted, Handover, NewSession, Pick, Purge, SessionFilter};
pub use staged::StagedReplacement;
pub use tasks::{Launch, NewTask};

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const DATABASE_FILE: &str = "drift.db";
/// Memory-map the database so hot reads skip the syscall per page.
const MMAP_SIZE_BYTES: i64 = 134_217_728;
/// How long a write waits on another connection before failing.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const WORKSPACE_COLUMNS: &str = "id, path, name, icon, last_used";

pub struct Store {
    conn: Mutex<Connection>,
    /// Open text and reasoning parts for transcript reads between disk checkpoints.
    streaming: Mutex<std::collections::HashMap<String, crate::session::types::PartRow>>,
}

impl Store {
    fn new(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
            streaming: Mutex::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub path: String,
    pub name: String,
    pub icon: String,
    pub last_used: i64,
}

pub fn open(dir: &Path) -> rusqlite::Result<Store> {
    std::fs::create_dir_all(dir).ok();
    open_file(&dir.join(DATABASE_FILE))
}

pub fn open_file(file: &Path) -> rusqlite::Result<Store> {
    let conn = Connection::open(file)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "mmap_size", MMAP_SIZE_BYTES)?;
    migrations::apply(&conn)?;
    Ok(Store::new(conn))
}

impl Store {
    /// The one connection. Hold the guard for the whole unit of work and no longer.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn workspace(&self, id: &str) -> rusqlite::Result<Option<Workspace>> {
        self.lock()
            .prepare_cached(&format!("SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id = ?1"))?
            .query_row([id], map_workspace)
            .optional()
    }

    pub fn workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE removed_at IS NULL ORDER BY last_used DESC"
        ))?;
        let rows = stmt.query_map([], map_workspace)?;
        rows.collect()
    }

    /// Adds a directory, or restores and touches the row that already holds it.
    pub fn add_workspace(&self, path: &str, name: &str, icon: &str) -> rusqlite::Result<Workspace> {
        let conn = self.lock();
        let existing: Option<String> = conn
            .prepare_cached(
                "SELECT id FROM workspace
                 WHERE LOWER(REPLACE(path, '\\', '/')) = LOWER(REPLACE(?1, '\\', '/'))
                 ORDER BY (removed_at IS NULL) DESC, last_used DESC LIMIT 1",
            )?
            .query_row([path], |row| row.get(0))
            .optional()?;
        let id = match existing {
            Some(id) => {
                conn.prepare_cached("UPDATE workspace SET removed_at = NULL, last_used = ?2 WHERE id = ?1")?
                    .execute((&id, now()))?;
                id
            }
            None => {
                let id = new_id();
                conn.prepare_cached(
                    "INSERT INTO workspace(id, path, name, icon, last_used) VALUES(?1, ?2, ?3, ?4, ?5)",
                )?
                .execute((&id, path, name, icon, now()))?;
                id
            }
        };
        let workspace = conn
            .prepare_cached(&format!("SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id = ?1"))?
            .query_row([&id], map_workspace)?;
        Ok(workspace)
    }
}

impl Store {
    /// The workspace's conversations, archived and subagents included; `None` when it is not a removed workspace.
    pub fn removed_workspace_sessions(&self, id: &str) -> rusqlite::Result<Option<Vec<String>>> {
        let conn = self.lock();
        let removed: Option<bool> = conn
            .prepare_cached("SELECT removed_at IS NOT NULL FROM workspace WHERE id = ?1")?
            .query_row([id], |row| row.get(0))
            .optional()?;
        if removed != Some(true) {
            return Ok(None);
        }
        let ids = conn
            .prepare_cached("SELECT id FROM session WHERE workspace_id = ?1")?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(ids))
    }

    /// Deletes every conversation of a removed workspace in one write; `None`, deleting nothing, once it is in use again.
    pub fn purge_removed_workspace(&self, id: &str) -> rusqlite::Result<Option<usize>> {
        sessions::transaction(&self.lock(), |conn| {
            let removed: Option<bool> = conn
                .prepare_cached("SELECT removed_at IS NOT NULL FROM workspace WHERE id = ?1")?
                .query_row([id], |row| row.get(0))
                .optional()?;
            if removed != Some(true) {
                return Ok(None);
            }
            Ok(Some(
                conn.prepare_cached("DELETE FROM session WHERE workspace_id = ?1")?
                    .execute([id])?,
            ))
        })
    }
}

fn map_workspace(row: &rusqlite::Row) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        icon: row.get(3)?,
        last_used: row.get(4)?,
    })
}

fn now() -> i64 {
    crate::id::now_ms()
}

fn new_id() -> String {
    crate::random_hex(12)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn store() -> Store {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        migrations::apply(&conn).unwrap();
        Store::new(conn)
    }

    #[test]
    fn add_then_list() {
        let store = store();
        let added = store.add_workspace("C:/repo", "repo", "").unwrap();
        assert_eq!(added.path, "C:/repo");
        assert_eq!(added.id.len(), 24);
        assert_eq!(store.workspaces().unwrap(), vec![added]);
    }

    #[test]
    fn same_directory_with_different_casing_or_slashes_is_one_workspace() {
        let store = store();
        let first = store.add_workspace("C:/Repo", "repo", "").unwrap();
        let second = store.add_workspace("c:\\repo", "other", "").unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(store.workspaces().unwrap().len(), 1);
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrations::apply(&conn).unwrap();
        migrations::apply(&conn).unwrap();
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, migrations::LATEST);
    }

    #[test]
    fn widening_the_ending_check_keeps_the_endings_already_stored() {
        let conn = Connection::open_in_memory().unwrap();
        let widening = migrations::MIGRATIONS
            .iter()
            .position(|sql| sql.contains("RENAME COLUMN ended TO ending"))
            .unwrap();
        for (index, sql) in migrations::MIGRATIONS.iter().take(widening).enumerate() {
            conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {}; COMMIT;", index + 1))
                .unwrap();
        }
        conn.execute_batch("PRAGMA foreign_keys = OFF; INSERT INTO message(id, session_id, role, status, usage_json, cost, created_at, summary, ending) VALUES('m', 's', 'assistant', 'done', '{}', 0, 0, 0, 'length');").unwrap();
        migrations::apply(&conn).unwrap();
        let kept: String = conn
            .query_row("SELECT ending FROM message WHERE id = 'm'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, "length");
        conn.execute_batch("UPDATE message SET ending = 'limit' WHERE id = 'm';")
            .unwrap();
        assert!(
            conn.execute_batch("UPDATE message SET ending = 'other' WHERE id = 'm';")
                .is_err(),
            "the check still holds"
        );
    }
}
