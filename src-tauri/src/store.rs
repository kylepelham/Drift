mod settings;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// Columns selected by every workspace query, in the order read by map_workspace.
/// Keep both orders aligned to avoid assigning values to the wrong fields.
const WORKSPACE_COLUMNS: &str = "id, path, name, icon, last_used, removed_at";

/// Shell tables live in the engine's database and go through its single connection.
pub(crate) struct Store(Arc<drift_engine::store::Store>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Workspace {
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
pub(crate) struct ArchivedSession {
    pub session_id: String,
    pub workspace_id: String,
    pub archived_at: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServer {
    pub name: String,
    pub config: Value,
    pub updated_at: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpDecision {
    pub name: String,
    pub fingerprint: String,
    pub decision: String,
    pub decided_at: i64,
}

/// A browser or app signed in to Remote Access. Only the SHA-256 of its session token is stored.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteDevice {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub token_hash: String,
    pub method: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

#[derive(Clone)]
pub(crate) struct McpState {
    pub servers: Vec<McpServer>,
    pub decisions: Vec<McpDecision>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptOverride {
    pub key: String,
    pub value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<Value>,
    pub updated_at: i64,
}

#[cfg(test)]
pub(crate) fn open(dir: &Path) -> rusqlite::Result<Store> {
    attach(Arc::new(drift_engine::store::open(dir)?))
}

#[cfg(test)]
fn open_at(file: &Path) -> rusqlite::Result<Store> {
    attach(Arc::new(drift_engine::store::open_file(file)?))
}

/// Creates the shell's tables on the engine's connection. The engine owns `workspace`.
pub(crate) fn attach(engine: Arc<drift_engine::store::Store>) -> rusqlite::Result<Store> {
    let store = Store(engine);
    let conn = store.0.lock();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_meta(
            session_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            archived_at INTEGER
        ) STRICT;
        CREATE INDEX IF NOT EXISTS idx_session_meta_workspace ON session_meta(workspace_id);
        CREATE TABLE IF NOT EXISTS mcp_server(
            name TEXT PRIMARY KEY,
            config_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS mcp_decision(
            fingerprint TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            decision TEXT NOT NULL CHECK(decision IN ('approved', 'rejected')),
            decided_at INTEGER NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS mcp_state(
            id INTEGER PRIMARY KEY CHECK(id = 1),
            generation INTEGER NOT NULL,
            materialized_generation INTEGER NOT NULL
        ) STRICT;
        INSERT OR IGNORE INTO mcp_state(id, generation, materialized_generation) VALUES(1, 0, -1);
        CREATE TABLE IF NOT EXISTS prompt_override(
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            original_json TEXT,
            updated_at INTEGER NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS app_setting(
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS remote_access(
            id INTEGER PRIMARY KEY CHECK(id = 1),
            enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
            token TEXT NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS remote_device(
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            method TEXT NOT NULL CHECK(method IN ('link', 'password')),
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL
        ) STRICT;",
    )?;
    // The removed model-recovery workflow used this table.
    conn.execute("DROP TABLE IF EXISTS recoverable_interruption", [])?;
    collapse_duplicate_workspaces(&conn)?;
    drop(conn);
    Ok(store)
}

/// Collapses duplicate paths that differ in slash direction or casing, including old forward-slash imports.
/// Prefers active rows, then rows with icons, then the most recently used.
fn collapse_duplicate_workspaces(conn: &Connection) -> rusqlite::Result<()> {
    let losers: Vec<(String, String)> = conn
        .prepare(
            "SELECT id, winner FROM (
                SELECT id,
                       FIRST_VALUE(id) OVER (
                           PARTITION BY LOWER(REPLACE(path, '\\', '/'))
                           ORDER BY (removed_at IS NULL) DESC, (icon <> '') DESC, last_used DESC, id
                       ) AS winner
                FROM workspace
            ) WHERE id <> winner",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    for (loser, winner) in &losers {
        conn.execute(
            "UPDATE session_meta SET workspace_id = ?2 WHERE workspace_id = ?1",
            (loser, winner),
        )?;
        conn.execute("DELETE FROM workspace WHERE id = ?1", [loser])?;
    }

    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

impl Store {
    pub(crate) fn import_opencode_workspaces(&self, database: &Path) -> rusqlite::Result<usize> {
        if !database.is_file() {
            return Ok(0);
        }
        let conn = self.0.lock();
        conn.execute(
            "ATTACH DATABASE ?1 AS opencode_import",
            [database.to_string_lossy().as_ref()],
        )?;
        let temp_prefix = format!(
            "{}/",
            std::env::temp_dir()
                .to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
        );
        // Imported temp-dir rows are scratch artifacts; the id/worktree match spares user rows.
        conn.execute(
            "DELETE FROM workspace
              WHERE removed_at IS NULL AND icon = ''
                AND (REPLACE(path, '\\', '/') LIKE (?1 || '%')
                  OR REPLACE(path, '\\', '/') LIKE '%/AppData/Local/Temp/%'
                  OR REPLACE(path, '\\', '/') LIKE '/tmp/%')
                AND EXISTS (
                    SELECT 1 FROM opencode_import.project project
                    WHERE project.id = workspace.id AND project.worktree = workspace.path
               )",
            params![temp_prefix],
        )?;
        // Import only new non-temporary folders, preserving opencode's name or using the final path segment.
        let result = conn.execute(
            "INSERT OR IGNORE INTO workspace(id, path, name, icon, last_used)
             SELECT project.id, project.worktree,
                    COALESCE(NULLIF(project.name, ''), (
                        SELECT NULLIF(REPLACE(trimmed, RTRIM(trimmed, REPLACE(trimmed, '/', '')), ''), '')
                        FROM (SELECT RTRIM(REPLACE(project.worktree, '\\', '/'), '/') AS trimmed)
                    ), project.worktree), '',
                    MAX(COALESCE(session.time_updated, project.time_updated, 0))
              FROM opencode_import.project project
              JOIN opencode_import.session session ON session.project_id = project.id
              WHERE project.worktree <> '' AND project.worktree <> '/'
                AND REPLACE(project.worktree, '\\', '/') NOT LIKE (?1 || '%')
                AND REPLACE(project.worktree, '\\', '/') NOT LIKE '%/AppData/Local/Temp/%'
                AND REPLACE(project.worktree, '\\', '/') NOT LIKE '/tmp/%'
                AND NOT EXISTS (
                   SELECT 1 FROM workspace existing
                   WHERE LOWER(REPLACE(existing.path, '\\', '/')) = LOWER(REPLACE(project.worktree, '\\', '/'))
               )
             GROUP BY project.id, project.worktree, project.name",
            params![temp_prefix],
        );
        let _ = conn.execute_batch("DETACH DATABASE opencode_import");
        result
    }

    /// Workspaces still in use, most recently opened first.
    pub(crate) fn workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        self.query_workspaces("WHERE removed_at IS NULL ORDER BY last_used DESC")
    }

    /// Soft-deleted workspaces awaiting purge, most recently removed first.
    pub(crate) fn removed_workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        self.query_workspaces("WHERE removed_at IS NOT NULL ORDER BY removed_at DESC")
    }

    fn query_workspaces(&self, filter: &str) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock();
        let mut stmt = conn.prepare_cached(&format!("SELECT {WORKSPACE_COLUMNS} FROM workspace {filter}"))?;
        let rows = stmt.query_map([], map_workspace)?;
        rows.collect()
    }

    pub(crate) fn add_workspace(&self, id: &str, path: &str, name: &str, icon: &str) -> rusqlite::Result<Workspace> {
        let conn = self.0.lock();
        // Restore an existing directory row even when the supplied path changes casing or slash direction.
        let existing: Option<String> = conn
            .prepare_cached(
                "SELECT id FROM workspace
                 WHERE LOWER(REPLACE(path, '\\', '/')) = LOWER(REPLACE(?1, '\\', '/'))
                 ORDER BY (removed_at IS NULL) DESC, last_used DESC LIMIT 1",
            )?
            .query_row([path], |row| row.get(0))
            .optional()?;

        let target = match existing {
            Some(found) => {
                conn.prepare_cached("UPDATE workspace SET removed_at = NULL, last_used = ?2 WHERE id = ?1")?
                    .execute((&found, now()))?;
                found
            }
            None => {
                conn.prepare_cached(
                    "INSERT INTO workspace(id, path, name, icon, last_used) VALUES(?1, ?2, ?3, ?4, ?5)",
                )?
                .execute((id, path, name, icon, now()))?;
                id.to_string()
            }
        };

        let workspace = conn
            .prepare_cached(&format!("SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id = ?1"))?
            .query_row([&target], map_workspace)?;
        Ok(workspace)
    }

    pub(crate) fn save_workspace(&self, id: &str, path: &str, name: &str, icon: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock();
        // Editing a path onto another row's directory merges that row into this one.
        let clashes: Vec<String> = conn
            .prepare_cached(
                "SELECT id FROM workspace
                 WHERE id <> ?1 AND LOWER(REPLACE(path, '\\', '/')) = LOWER(REPLACE(?2, '\\', '/'))",
            )?
            .query_map((id, path), |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        for clash in &clashes {
            conn.execute(
                "UPDATE session_meta SET workspace_id = ?2 WHERE workspace_id = ?1",
                (clash, id),
            )?;
            conn.execute("DELETE FROM workspace WHERE id = ?1", [clash])?;
        }
        conn.prepare_cached(
            "INSERT INTO workspace(id, path, name, icon, last_used) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET path = ?2, name = ?3, icon = ?4",
        )?
        .execute((id, path, name, icon, now()))?;
        Ok(())
    }

    pub(crate) fn touch_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock();
        conn.prepare_cached("UPDATE workspace SET last_used = ?2 WHERE id = ?1")?
            .execute((id, now()))?;
        Ok(())
    }

    pub(crate) fn remove_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock();
        conn.prepare_cached("UPDATE workspace SET removed_at = ?2 WHERE id = ?1")?
            .execute((id, now()))?;
        Ok(())
    }

    /// Workspaces removed before before whose directory no active workspace uses.
    /// Collapses stale duplicates of active directories instead of returning them for retention deletion.
    /// This prevents retention from deleting sessions still on the sidebar.
    pub(crate) fn expired_removed_workspaces(&self, before: i64) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock();
        let duplicates: Vec<(String, String)> = conn
            .prepare_cached(
                "SELECT removed.id, active.id FROM workspace removed
                 JOIN workspace active
                   ON active.removed_at IS NULL
                  AND LOWER(REPLACE(active.path, '\\', '/')) = LOWER(REPLACE(removed.path, '\\', '/'))
                 WHERE removed.removed_at IS NOT NULL",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (removed, active) in &duplicates {
            conn.execute(
                "UPDATE session_meta SET workspace_id = ?2 WHERE workspace_id = ?1",
                (removed, active),
            )?;
            conn.execute("DELETE FROM workspace WHERE id = ?1", [removed])?;
        }
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace expired
             WHERE removed_at IS NOT NULL AND removed_at < ?1
               AND NOT EXISTS (
                   SELECT 1 FROM workspace active
                   WHERE active.removed_at IS NULL
                     AND LOWER(REPLACE(active.path, '\\', '/')) = LOWER(REPLACE(expired.path, '\\', '/'))
               )"
        ))?;
        let rows = stmt.query_map([before], map_workspace)?;
        rows.collect()
    }

    /// Drops an expired removed workspace and returns whether its row was removed.
    /// Call only after engine sessions are gone, otherwise startup import can restore the row from leftovers.
    pub(crate) fn forget_workspace(&self, id: &str) -> rusqlite::Result<bool> {
        let conn = self.0.lock();
        conn.prepare_cached("DELETE FROM session_meta WHERE workspace_id = ?1")?
            .execute([id])?;
        let removed = conn
            .prepare_cached("DELETE FROM workspace WHERE id = ?1 AND removed_at IS NOT NULL")?
            .execute([id])?;
        Ok(removed > 0)
    }

    pub(crate) fn archived(&self) -> rusqlite::Result<Vec<ArchivedSession>> {
        let conn = self.0.lock();
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

    pub(crate) fn unarchive_session(&self, session_id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock();
        conn.prepare_cached("DELETE FROM session_meta WHERE session_id = ?1")?
            .execute([session_id])?;
        Ok(())
    }

    pub(crate) fn archive_session(&self, session_id: &str, workspace_id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock();
        conn.prepare_cached(
            "INSERT INTO session_meta(session_id, workspace_id, archived_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET workspace_id = ?2, archived_at = ?3",
        )?
        .execute((session_id, workspace_id, now()))?;
        Ok(())
    }

    /// Lists archived sessions whose retention window has elapsed without deleting their tombstones.
    /// Tombstones are removed via unarchive_session only after the engine confirms session deletion.
    /// Failed deletions are therefore retried on a later purge.
    pub(crate) fn expired_archived(&self, before: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.0.lock();
        let mut stmt = conn
            .prepare_cached("SELECT session_id FROM session_meta WHERE archived_at IS NOT NULL AND archived_at < ?1")?;
        let rows = stmt.query_map([before], |row| row.get(0))?;
        rows.collect()
    }

    pub(crate) fn mcp_state(&self) -> rusqlite::Result<McpState> {
        let conn = self.0.lock();
        let servers = conn
            .prepare_cached("SELECT name, config_json, updated_at FROM mcp_server ORDER BY name COLLATE NOCASE")?
            .query_map([], |row| {
                let raw: String = row.get(1)?;
                let config = serde_json::from_str(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(raw.len(), rusqlite::types::Type::Text, Box::new(error))
                })?;
                Ok(McpServer {
                    name: row.get(0)?,
                    config,
                    updated_at: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let decisions = conn
            .prepare_cached("SELECT name, fingerprint, decision, decided_at FROM mcp_decision ORDER BY decided_at")?
            .query_map([], |row| {
                Ok(McpDecision {
                    name: row.get(0)?,
                    fingerprint: row.get(1)?,
                    decision: row.get(2)?,
                    decided_at: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(McpState { servers, decisions })
    }
}

/// Reads a workspace row; column positions must match WORKSPACE_COLUMNS.
fn map_workspace(row: &rusqlite::Row) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        icon: row.get(3)?,
        last_used: row.get(4)?,
        removed_at: row.get(5)?,
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
