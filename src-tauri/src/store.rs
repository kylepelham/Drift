use rusqlite::Connection;
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
            last_used INTEGER NOT NULL DEFAULT 0
        ) STRICT;
        CREATE TABLE IF NOT EXISTS session_meta(
            session_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            archived_at INTEGER
        ) STRICT;
        CREATE INDEX IF NOT EXISTS idx_session_meta_workspace ON session_meta(workspace_id);",
    )?;
    Ok(Store(Mutex::new(conn)))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock().unwrap();
        let mut stmt =
            conn.prepare_cached("SELECT id, path, name, icon, last_used FROM workspace ORDER BY last_used DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(Workspace {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                icon: row.get(3)?,
                last_used: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn save_workspace(&self, id: &str, path: &str, name: &str, icon: &str) -> rusqlite::Result<()> {
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

    pub fn delete_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached("DELETE FROM workspace WHERE id = ?1")?.execute([id])?;
        conn.prepare_cached("DELETE FROM session_meta WHERE workspace_id = ?1")?
            .execute([id])?;
        Ok(())
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
        conn.prepare_cached("DELETE FROM session_meta WHERE archived_at IS NOT NULL AND archived_at < ?1")?
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
        std::fs::create_dir_all(&dir).unwrap();
        let store = open(&dir).unwrap();

        store.save_workspace("w1", "S:/proj", "Proj", "P").unwrap();
        store.save_workspace("w1", "S:/proj", "Renamed", "R").unwrap();
        let list = store.workspaces().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Renamed");

        store.archive_session("s1", "w1").unwrap();
        store.archive_session("s2", "w1").unwrap();
        assert_eq!(store.archived().unwrap().len(), 2);

        let purged = store.purge_archived(now() + 1000).unwrap();
        assert_eq!(purged.len(), 2);
        assert!(store.archived().unwrap().is_empty());

        store.delete_workspace("w1").unwrap();
        assert!(store.workspaces().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
