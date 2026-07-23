use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

pub struct Store(Mutex<Connection>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub path: String,
    pub name: String,
    pub icon: String,
    pub last_used: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedSession {
    pub session_id: String,
    pub workspace_id: String,
    pub archived_at: i64,
}

pub fn open(dir: &Path) -> rusqlite::Result<Store> {
    std::fs::create_dir_all(dir).ok();
    open_at(&dir.join("drift.db"))
}

fn open_at(file: &Path) -> rusqlite::Result<Store> {
    let conn = Connection::open(file)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "mmap_size", 134_217_728)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace(
            id TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            icon TEXT NOT NULL DEFAULT '',
            last_used INTEGER NOT NULL DEFAULT 0,
            removed_at INTEGER
        ) STRICT;
        CREATE TABLE IF NOT EXISTS session_meta(
            session_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            archived_at INTEGER
        ) STRICT;
        CREATE INDEX IF NOT EXISTS idx_session_meta_workspace ON session_meta(workspace_id);",
    )?;
    let _ = conn.execute("ALTER TABLE workspace ADD COLUMN removed_at INTEGER", []);
    Ok(Store(Mutex::new(conn)))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn import_opencode_workspaces(&self, database: &Path) -> rusqlite::Result<usize> {
        if !database.is_file() {
            return Ok(0);
        }
        let conn = self.0.lock().unwrap();
        conn.execute(
            "ATTACH DATABASE ?1 AS opencode_import",
            [database.to_string_lossy().as_ref()],
        )?;
        let result = conn.execute(
            "INSERT OR IGNORE INTO workspace(id, path, name, icon, last_used)
             SELECT project.id, project.worktree,
                    COALESCE(NULLIF(project.name, ''), project.worktree), '',
                    MAX(COALESCE(session.time_updated, project.time_updated, 0))
             FROM opencode_import.project project
             LEFT JOIN opencode_import.session session ON session.project_id = project.id
             WHERE project.worktree <> '' AND project.worktree <> '/'
             GROUP BY project.id, project.worktree, project.name",
            [],
        );
        let _ = conn.execute_batch("DETACH DATABASE opencode_import");
        result
    }

    pub fn workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, path, name, icon, last_used, removed_at FROM workspace WHERE removed_at IS NULL ORDER BY last_used DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Workspace {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                icon: row.get(3)?,
                last_used: row.get(4)?,
                removed_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn removed_workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, path, name, icon, last_used, removed_at FROM workspace WHERE removed_at IS NOT NULL ORDER BY removed_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Workspace {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                icon: row.get(3)?,
                last_used: row.get(4)?,
                removed_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn add_workspace(
        &self,
        id: &str,
        path: &str,
        name: &str,
        icon: &str,
    ) -> rusqlite::Result<Workspace> {
        let conn = self.0.lock().unwrap();
        let existing: Option<String> = conn
            .prepare_cached("SELECT id FROM workspace WHERE path = ?1")?
            .query_row([path], |row| row.get(0))
            .optional()?;
        let target = match existing {
            Some(found) => {
                conn.prepare_cached(
                    "UPDATE workspace SET removed_at = NULL, last_used = ?2 WHERE id = ?1",
                )?
                .execute((&found, now()))?;
                found
            }
            None => {
                conn.prepare_cached("INSERT INTO workspace(id, path, name, icon, last_used) VALUES(?1, ?2, ?3, ?4, ?5)")?
                    .execute((id, path, name, icon, now()))?;
                id.to_string()
            }
        };
        let workspace = conn
            .prepare_cached(
                "SELECT id, path, name, icon, last_used, removed_at FROM workspace WHERE id = ?1",
            )?
            .query_row([&target], |row| {
                Ok(Workspace {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    name: row.get(2)?,
                    icon: row.get(3)?,
                    last_used: row.get(4)?,
                    removed_at: row.get(5)?,
                })
            })?;
        Ok(workspace)
    }

    pub fn save_workspace(
        &self,
        id: &str,
        path: &str,
        name: &str,
        icon: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached(
            "INSERT INTO workspace(id, path, name, icon, last_used) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET path = ?2, name = ?3, icon = ?4",
        )?
        .execute((id, path, name, icon, now()))?;
        Ok(())
    }

    pub fn touch_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached("UPDATE workspace SET last_used = ?2 WHERE id = ?1")?
            .execute((id, now()))?;
        Ok(())
    }

    pub fn remove_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached("UPDATE workspace SET removed_at = ?2 WHERE id = ?1")?
            .execute((id, now()))?;
        Ok(())
    }

    pub fn purge_removed_workspaces(&self, before: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let rows: Vec<(String, String)> = conn
            .prepare_cached(
                "SELECT id, path FROM workspace WHERE removed_at IS NOT NULL AND removed_at < ?1",
            )?
            .query_map([before], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (id, _) in &rows {
            conn.prepare_cached("DELETE FROM session_meta WHERE workspace_id = ?1")?
                .execute([id])?;
            conn.prepare_cached("DELETE FROM workspace WHERE id = ?1")?
                .execute([id])?;
        }
        Ok(rows.into_iter().map(|(_, path)| path).collect())
    }

    pub fn archived(&self) -> rusqlite::Result<Vec<ArchivedSession>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT session_id, workspace_id, archived_at FROM session_meta WHERE archived_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ArchivedSession {
                session_id: row.get(0)?,
                workspace_id: row.get(1)?,
                archived_at: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn unarchive_session(&self, session_id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached("DELETE FROM session_meta WHERE session_id = ?1")?
            .execute([session_id])?;
        Ok(())
    }

    pub fn archive_session(&self, session_id: &str, workspace_id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached(
            "INSERT INTO session_meta(session_id, workspace_id, archived_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET workspace_id = ?2, archived_at = ?3",
        )?
        .execute((session_id, workspace_id, now()))?;
        Ok(())
    }

    pub fn purge_archived(&self, before: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let ids: Vec<String> = conn
            .prepare_cached("SELECT session_id FROM session_meta WHERE archived_at IS NOT NULL AND archived_at < ?1")?
            .query_map([before], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        conn.prepare_cached(
            "DELETE FROM session_meta WHERE archived_at IS NOT NULL AND archived_at < ?1",
        )?
        .execute([before])?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("drift-store-test-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let store = open(&dir).unwrap();

        let created = store.add_workspace("w1", "S:/proj", "Proj", "").unwrap();
        assert_eq!(created.id, "w1");
        store
            .save_workspace("w1", "S:/moved", "Renamed", "R")
            .unwrap();
        assert_eq!(store.workspaces().unwrap()[0].name, "Renamed");
        assert_eq!(store.workspaces().unwrap()[0].path, "S:/moved");

        store.archive_session("s1", "w1").unwrap();
        store.archive_session("s2", "w1").unwrap();
        assert_eq!(store.archived().unwrap().len(), 2);
        store.unarchive_session("s1").unwrap();
        assert_eq!(store.archived().unwrap().len(), 1);
        let purged = store.purge_archived(now() + 1000).unwrap();
        assert_eq!(purged.len(), 1);
        assert!(store.archived().unwrap().is_empty());

        store.remove_workspace("w1").unwrap();
        assert!(store.workspaces().unwrap().is_empty());
        assert_eq!(store.removed_workspaces().unwrap().len(), 1);

        let restored = store.add_workspace("w2", "S:/moved", "Ignored", "").unwrap();
        assert_eq!(restored.id, "w1");
        assert_eq!(restored.name, "Renamed");
        assert_eq!(store.workspaces().unwrap().len(), 1);

        store.remove_workspace("w1").unwrap();
        let paths = store.purge_removed_workspaces(now() + 1000).unwrap();
        assert_eq!(paths, vec!["S:/moved".to_string()]);
        assert!(store.workspaces().unwrap().is_empty());
        assert!(
            store
                .add_workspace("w3", "S:/moved", "Fresh", "")
                .unwrap()
                .id
                == "w3"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn imports_opencode_projects_without_overwriting_drift_metadata() {
        let dir = std::env::temp_dir().join(format!("drift-import-test-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("opencode.db");
        let conn = Connection::open(&source).unwrap();
        conn.execute_batch(
            "CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT, name TEXT, time_updated INTEGER);
             CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, time_updated INTEGER);
             INSERT INTO project VALUES('p1', 'S:/one', 'One', 10);
             INSERT INTO project VALUES('global', '/', 'Global', 20);
             INSERT INTO session VALUES('s1', 'p1', 30);",
        )
        .unwrap();
        drop(conn);

        let store = open_at(&dir.join("drift.db")).unwrap();
        assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 1);
        assert_eq!(store.workspaces().unwrap()[0].path, "S:/one");
        store.save_workspace("p1", "S:/one", "Custom", "C").unwrap();
        assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 0);
        assert_eq!(store.workspaces().unwrap()[0].name, "Custom");
        std::fs::remove_dir_all(&dir).ok();
    }
}
