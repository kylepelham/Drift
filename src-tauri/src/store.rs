use rusqlite::{params, types::Type, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

const DATABASE_FILE: &str = "drift.db";
/// The column list every workspace query selects, in the order `map_workspace` reads them.
/// Keep the two in step: reordering one without the other silently mixes up the fields.
const WORKSPACE_COLUMNS: &str = "id, path, name, icon, last_used, removed_at";
/// Let SQLite memory-map up to 128 MiB of the database. Reads then avoid a syscall per page, which
/// matters because the workspace and session lists are re-read on nearly every UI interaction.
const MMAP_SIZE_BYTES: i64 = 134_217_728;

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

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverableInterruption {
    pub session_id: String,
    pub identity: String,
    pub workspace_id: Option<String>,
    pub directory: String,
    pub thread_title: String,
    pub parent_session_id: Option<String>,
    pub provider_id: String,
    pub model_id: String,
    pub kind: String,
    pub reason: String,
    pub error_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub dismissed_at: Option<i64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    pub config: Value,
    pub updated_at: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDecision {
    pub name: String,
    pub fingerprint: String,
    pub decision: String,
    pub decided_at: i64,
}

#[derive(Clone)]
pub struct McpState {
    pub generation: i64,
    pub servers: Vec<McpServer>,
    pub decisions: Vec<McpDecision>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptOverride {
    pub key: String,
    pub value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<Value>,
    pub updated_at: i64,
}

pub fn open(dir: &Path) -> rusqlite::Result<Store> {
    std::fs::create_dir_all(dir).ok();
    open_at(&dir.join(DATABASE_FILE))
}

fn open_at(file: &Path) -> rusqlite::Result<Store> {
    let conn = Connection::open(file)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "mmap_size", MMAP_SIZE_BYTES)?;
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
        CREATE TABLE IF NOT EXISTS recoverable_interruption(
            session_id TEXT NOT NULL,
            identity TEXT NOT NULL,
            workspace_id TEXT,
            directory TEXT NOT NULL,
            thread_title TEXT NOT NULL,
            parent_session_id TEXT,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('usage', 'rate_limit', 'unavailable', 'provider_auth', 'transient')),
            reason TEXT NOT NULL,
            error_name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            dismissed_at INTEGER,
            PRIMARY KEY(session_id, identity)
        ) STRICT;
        CREATE TABLE IF NOT EXISTS app_setting(
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS remote_access(
            id INTEGER PRIMARY KEY CHECK(id = 1),
            enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
            token TEXT NOT NULL
        ) STRICT;",
    )?;
    // Migration for databases created before `removed_at` existed. On any database created by the
    // CREATE TABLE above the column is already there and this fails with "duplicate column name",
    // which is why the error is deliberately discarded rather than propagated.
    let _ = conn.execute("ALTER TABLE workspace ADD COLUMN removed_at INTEGER", []);
    collapse_duplicate_workspaces(&conn)?;
    Ok(Store(Mutex::new(conn)))
}

/// Collapses rows whose paths differ only in slash direction or casing (old imports used
/// forward slashes), keeping active > iconed > most recently used.
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
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn app_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM app_setting WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn initialize_app_setting(&self, key: &str, value: &str) -> rusqlite::Result<String> {
        let mut conn = self.0.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO app_setting(key, value) VALUES(?1, ?2)",
            params![key, value],
        )?;
        let stored = transaction.query_row(
            "SELECT value FROM app_setting WHERE key = ?1",
            [key],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(stored)
    }

    pub fn save_app_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO app_setting(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_app_setting(&self, key: &str) -> rusqlite::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM app_setting WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn dictation_enabled(&self) -> rusqlite::Result<bool> {
        let value: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM app_setting WHERE key = 'dictation_enabled'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.as_deref() == Some("true"))
    }

    pub fn save_dictation_enabled(&self, enabled: bool) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO app_setting(key, value) VALUES('dictation_enabled', ?1)
             ON CONFLICT(key) DO UPDATE SET value = ?1",
            [if enabled { "true" } else { "false" }],
        )?;
        Ok(())
    }

    pub fn remote_access(&self) -> rusqlite::Result<Option<(bool, String)>> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT enabled, token FROM remote_access WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?)),
            )
            .optional()
    }

    pub fn save_remote_access(&self, enabled: bool, token: &str) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO remote_access(id, enabled, token) VALUES(1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled = ?1, token = ?2",
            params![enabled as i64, token],
        )?;
        Ok(())
    }

    pub fn prompt_overrides(&self) -> rusqlite::Result<Vec<PromptOverride>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT key, value_json, original_json, updated_at FROM prompt_override ORDER BY key",
        )?;
        let rows = stmt.query_map([], |row| {
            let value: String = row.get(1)?;
            let original: Option<String> = row.get(2)?;
            Ok(PromptOverride {
                key: row.get(0)?,
                value: serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
                })?,
                original: original
                    .map(|item| {
                        serde_json::from_str(&item).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                Type::Text,
                                Box::new(error),
                            )
                        })
                    })
                    .transpose()?,
                updated_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn save_prompt_override(
        &self,
        key: &str,
        value: &Value,
        original: Option<&Value>,
    ) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached(
            "INSERT INTO prompt_override(key, value_json, original_json, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET value_json = ?2,
               original_json = COALESCE(prompt_override.original_json, ?3), updated_at = ?4",
        )?
        .execute(params![
            key,
            serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
            original.map(|item| serde_json::to_string(item).unwrap_or_else(|_| "null".into())),
            now()
        ])?;
        Ok(())
    }

    pub fn reset_prompt_override(&self, key: &str) -> rusqlite::Result<()> {
        self.0
            .lock()
            .unwrap()
            .prepare_cached("DELETE FROM prompt_override WHERE key = ?1")?
            .execute([key])?;
        Ok(())
    }

    pub fn import_opencode_workspaces(&self, database: &Path) -> rusqlite::Result<usize> {
        if !database.is_file() {
            return Ok(0);
        }
        let conn = self.0.lock().unwrap();
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
        // Skip temp directories and any canonical path that already has a row, active or removed.
        let result = conn.execute(
            "INSERT OR IGNORE INTO workspace(id, path, name, icon, last_used)
             SELECT project.id, project.worktree,
                    COALESCE(NULLIF(project.name, ''), project.worktree), '',
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
    pub fn workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        self.query_workspaces("WHERE removed_at IS NULL ORDER BY last_used DESC")
    }

    /// Soft-deleted workspaces awaiting purge, most recently removed first.
    pub fn removed_workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        self.query_workspaces("WHERE removed_at IS NOT NULL ORDER BY removed_at DESC")
    }

    fn query_workspaces(&self, filter: &str) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace {filter}"
        ))?;
        let rows = stmt.query_map([], map_workspace)?;
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
        // Canonical match so re-adding a directory restores its row instead of minting a variant.
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
            .prepare_cached(&format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id = ?1"
            ))?
            .query_row([&target], map_workspace)?;
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

    /// Workspaces removed before `before` whose directory no active workspace uses. Removed rows
    /// that still match an active directory are stale duplicates: collapsed here, never returned,
    /// so retention can't delete sessions that are still on the sidebar.
    pub fn expired_removed_workspaces(&self, before: i64) -> rusqlite::Result<Vec<Workspace>> {
        let conn = self.0.lock().unwrap();
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

    /// Drops an expired removed workspace. Only call after its engine sessions are gone, or the
    /// startup import resurrects the row from the leftovers.
    pub fn forget_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.prepare_cached("DELETE FROM session_meta WHERE workspace_id = ?1")?
            .execute([id])?;
        conn.prepare_cached("DELETE FROM workspace WHERE id = ?1 AND removed_at IS NOT NULL")?
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

    /// Archived sessions whose retention window has lapsed. Non-destructive: each row is the
    /// deletion tombstone and is only dropped via `unarchive_session` once the engine confirms
    /// the session is gone, so a failed engine deletion is retried on a later purge.
    pub fn expired_archived(&self, before: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT session_id FROM session_meta WHERE archived_at IS NOT NULL AND archived_at < ?1",
        )?;
        let rows = stmt.query_map([before], |row| row.get(0))?;
        rows.collect()
    }

    pub fn interruptions(&self) -> rusqlite::Result<Vec<RecoverableInterruption>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT session_id, identity, workspace_id, directory, thread_title, parent_session_id,
                    provider_id, model_id, kind, reason, error_name, created_at, updated_at, dismissed_at
             FROM recoverable_interruption ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RecoverableInterruption {
                session_id: row.get(0)?,
                identity: row.get(1)?,
                workspace_id: row.get(2)?,
                directory: row.get(3)?,
                thread_title: row.get(4)?,
                parent_session_id: row.get(5)?,
                provider_id: row.get(6)?,
                model_id: row.get(7)?,
                kind: row.get(8)?,
                reason: row.get(9)?,
                error_name: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                dismissed_at: row.get(13)?,
            })
        })?;
        rows.collect()
    }

    pub fn save_interruption(&self, item: &RecoverableInterruption) -> rusqlite::Result<()> {
        let mut conn = self.0.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction
            .prepare_cached(
                "DELETE FROM recoverable_interruption WHERE session_id = ?1 AND identity <> ?2",
            )?
            .execute((&item.session_id, &item.identity))?;
        transaction.prepare_cached(
            "INSERT INTO recoverable_interruption(
                session_id, identity, workspace_id, directory, thread_title, parent_session_id,
                provider_id, model_id, kind, reason, error_name, created_at, updated_at, dismissed_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(session_id, identity) DO UPDATE SET
                workspace_id = ?3, directory = ?4, thread_title = ?5, parent_session_id = ?6,
                provider_id = ?7, model_id = ?8, kind = ?9, reason = ?10, error_name = ?11,
                updated_at = ?13, dismissed_at = COALESCE(recoverable_interruption.dismissed_at, ?14)",
        )?.execute(params![
            item.session_id,
            item.identity,
            item.workspace_id,
            item.directory,
            item.thread_title,
            item.parent_session_id,
            item.provider_id,
            item.model_id,
            item.kind,
            item.reason,
            item.error_name,
            item.created_at,
            item.updated_at,
            item.dismissed_at,
        ])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn dismiss_interruption(
        &self,
        session_id: &str,
        identity: &str,
        dismissed_at: i64,
    ) -> rusqlite::Result<()> {
        self.0.lock().unwrap().prepare_cached(
            "UPDATE recoverable_interruption SET dismissed_at = ?3 WHERE session_id = ?1 AND identity = ?2",
        )?.execute((session_id, identity, dismissed_at))?;
        Ok(())
    }

    pub fn clear_interruptions(&self, session_id: &str) -> rusqlite::Result<()> {
        self.0
            .lock()
            .unwrap()
            .prepare_cached("DELETE FROM recoverable_interruption WHERE session_id = ?1")?
            .execute([session_id])?;
        Ok(())
    }

    pub fn mcp_state(&self) -> rusqlite::Result<McpState> {
        let conn = self.0.lock().unwrap();
        let generation = current_mcp_generation(&conn)?;
        let servers = conn
            .prepare_cached(
                "SELECT name, config_json, updated_at FROM mcp_server ORDER BY name COLLATE NOCASE",
            )?
            .query_map([], |row| {
                let raw: String = row.get(1)?;
                let config = serde_json::from_str(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        raw.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(McpServer {
                    name: row.get(0)?,
                    config,
                    updated_at: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let decisions = conn
            .prepare_cached(
                "SELECT name, fingerprint, decision, decided_at FROM mcp_decision ORDER BY decided_at",
            )?
            .query_map([], |row| {
                Ok(McpDecision {
                    name: row.get(0)?,
                    fingerprint: row.get(1)?,
                    decision: row.get(2)?,
                    decided_at: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(McpState {
            generation,
            servers,
            decisions,
        })
    }

    pub fn save_mcp_server(
        &self,
        name: &str,
        previous: Option<&str>,
        config: &Value,
    ) -> rusqlite::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        if let Some(previous) = previous.filter(|previous| *previous != name) {
            tx.execute("DELETE FROM mcp_server WHERE name = ?1", [previous])?;
        }
        tx.execute(
            "INSERT INTO mcp_server(name, config_json, updated_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET config_json = ?2, updated_at = ?3",
            params![name, serde_json::to_string(config).unwrap(), now()],
        )?;
        next_mcp_generation(&tx)?;
        let generation = current_mcp_generation(&tx)?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn remove_mcp_server(&self, name: &str) -> rusqlite::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM mcp_server WHERE name = ?1", [name])?;
        next_mcp_generation(&tx)?;
        let generation = current_mcp_generation(&tx)?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn decide_mcp(
        &self,
        name: &str,
        fingerprint: &str,
        decision: &str,
    ) -> rusqlite::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO mcp_decision(name, fingerprint, decision, decided_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(fingerprint) DO UPDATE SET name = ?1, decision = ?3, decided_at = ?4",
            params![name, fingerprint, decision, now()],
        )?;
        next_mcp_generation(&tx)?;
        let generation = current_mcp_generation(&tx)?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn revoke_mcp(&self, fingerprint: &str) -> rusqlite::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        if tx.execute(
            "DELETE FROM mcp_decision WHERE fingerprint = ?1",
            [fingerprint],
        )? != 1
        {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        next_mcp_generation(&tx)?;
        let generation = current_mcp_generation(&tx)?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn advance_mcp_generation(&self) -> rusqlite::Result<i64> {
        let conn = self.0.lock().unwrap();
        next_mcp_generation(&conn)?;
        current_mcp_generation(&conn)
    }

    pub fn restore_mcp_state(&self, state: &McpState) -> rusqlite::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM mcp_server", [])?;
        tx.execute("DELETE FROM mcp_decision", [])?;
        for server in &state.servers {
            tx.execute(
                "INSERT INTO mcp_server(name, config_json, updated_at) VALUES(?1, ?2, ?3)",
                params![
                    server.name,
                    serde_json::to_string(&server.config).unwrap(),
                    server.updated_at
                ],
            )?;
        }
        for decision in &state.decisions {
            tx.execute(
                "INSERT INTO mcp_decision(name, fingerprint, decision, decided_at) VALUES(?1, ?2, ?3, ?4)",
                params![
                    decision.name,
                    decision.fingerprint,
                    decision.decision,
                    decision.decided_at
                ],
            )?;
        }
        next_mcp_generation(&tx)?;
        let generation = current_mcp_generation(&tx)?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn mark_mcp_materialized(&self, generation: i64) -> rusqlite::Result<()> {
        let changed = self.0.lock().unwrap().execute(
            "UPDATE mcp_state SET materialized_generation = ?1 WHERE id = 1 AND generation = ?1",
            [generation],
        )?;
        if changed == 1 {
            return Ok(());
        }
        Err(rusqlite::Error::QueryReturnedNoRows)
    }
}

fn next_mcp_generation(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mcp_state SET generation = generation + 1 WHERE id = 1",
        [],
    )?;
    Ok(())
}

/// Reads the current MCP generation counter. Callers pass it back on the next mutation so a stale
/// frontend cannot overwrite a decision made since it last read.
fn current_mcp_generation(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
        row.get(0)
    })
}

/// Reads a workspace row. Column order must match `WORKSPACE_COLUMNS`.
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
