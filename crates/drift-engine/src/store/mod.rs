//! One SQLite database, one connection, one writer. Schema changes are numbered migrations.

mod migrations;
mod mcp;
mod queued;
mod sessions;
mod settings;
mod staged;
pub(crate) mod tasks;
mod todos;
mod tree;

pub use mcp::Renamed;
pub use queued::QueuedRow;
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

pub struct Store(Mutex<Connection>);

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
    Ok(Store(Mutex::new(conn)))
}

impl Store {
    /// The one connection. Hold the guard for the whole unit of work and no longer.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
        Store(Mutex::new(conn))
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
}
