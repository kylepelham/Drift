use rusqlite::{params, types::Type, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

const DATABASE_FILE: &str = "drift.db";
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
        ) STRICT;",
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
        conn.execute(
            "DELETE FROM workspace
             WHERE removed_at IS NULL AND icon = ''
               AND (REPLACE(path, '\\', '/') LIKE ?1 || '%' OR REPLACE(path, '\\', '/') LIKE '/tmp/%')
               AND EXISTS (
                   SELECT 1 FROM opencode_import.project project
                   WHERE project.id = workspace.id AND project.worktree = workspace.path
                     AND workspace.name = COALESCE(NULLIF(project.name, ''), project.worktree)
                     AND NOT EXISTS (
                         SELECT 1 FROM opencode_import.session session WHERE session.project_id = project.id
                     )
               )",
            params![temp_prefix],
        )?;
        let result = conn.execute(
            "INSERT OR IGNORE INTO workspace(id, path, name, icon, last_used)
             SELECT project.id, project.worktree,
                    COALESCE(NULLIF(project.name, ''), project.worktree), '',
                    MAX(COALESCE(session.time_updated, project.time_updated, 0))
             FROM opencode_import.project project
             JOIN opencode_import.session session ON session.project_id = project.id
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

    pub fn mcp_state(&self) -> rusqlite::Result<McpState> {
        let conn = self.0.lock().unwrap();
        let generation =
            conn.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
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
        let generation =
            tx.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn remove_mcp_server(&self, name: &str) -> rusqlite::Result<i64> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM mcp_server WHERE name = ?1", [name])?;
        next_mcp_generation(&tx)?;
        let generation =
            tx.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
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
        let generation =
            tx.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
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
        let generation =
            tx.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        tx.commit()?;
        Ok(generation)
    }

    pub fn advance_mcp_generation(&self) -> rusqlite::Result<i64> {
        let conn = self.0.lock().unwrap();
        next_mcp_generation(&conn)?;
        conn.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
            row.get(0)
        })
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
        let generation =
            tx.query_row("SELECT generation FROM mcp_state WHERE id = 1", [], |row| {
                row.get(0)
            })?;
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

        let restored = store
            .add_workspace("w2", "S:/moved", "Ignored", "")
            .unwrap();
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
        let value = serde_json::json!({ "prompt": "Drift prompt" });
        let original = serde_json::json!({ "prompt": "Original prompt" });
        store
            .save_prompt_override("agent:build", &value, Some(&original))
            .unwrap();
        let prompts = store.prompt_overrides().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].value, value);
        assert_eq!(prompts[0].original, Some(original));
        store.reset_prompt_override("agent:build").unwrap();
        assert!(store.prompt_overrides().unwrap().is_empty());
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
             INSERT INTO project VALUES('p2', '/tmp/project-directories', 'Temporary', 15);
             INSERT INTO project VALUES('p3', '/tmp/manual', 'Manual project', 16);
             INSERT INTO project VALUES('global', '/', 'Global', 20);
             INSERT INTO session VALUES('s1', 'p1', 30);",
        )
        .unwrap();
        drop(conn);

        let store = open_at(&dir.join("drift.db")).unwrap();
        store
            .add_workspace("p2", "/tmp/project-directories", "Temporary", "")
            .unwrap();
        store
            .add_workspace("manual", "/tmp/manual", "Manual", "")
            .unwrap();
        assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 1);
        let workspaces = store.workspaces().unwrap();
        assert_eq!(workspaces.len(), 2);
        assert!(workspaces
            .iter()
            .any(|workspace| workspace.path == "S:/one"));
        assert!(workspaces
            .iter()
            .any(|workspace| workspace.path == "/tmp/manual"));
        assert!(!workspaces
            .iter()
            .any(|workspace| workspace.path == "/tmp/project-directories"));
        store.save_workspace("p1", "S:/one", "Custom", "C").unwrap();
        assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 0);
        assert_eq!(
            store
                .workspaces()
                .unwrap()
                .into_iter()
                .find(|workspace| workspace.id == "p1")
                .unwrap()
                .name,
            "Custom"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mcp_decisions_are_global_and_survive_definition_changes() {
        let dir = std::env::temp_dir().join(format!("drift-mcp-store-test-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let store = open_at(&dir.join("drift.db")).unwrap();
        let first = serde_json::json!({ "type": "local", "command": ["one"] });
        let second = serde_json::json!({ "type": "local", "command": ["two"] });

        assert_eq!(store.save_mcp_server("server", None, &first).unwrap(), 1);
        assert_eq!(
            store
                .decide_mcp(
                    "server",
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "approved"
                )
                .unwrap(),
            2
        );
        store.save_mcp_server("server", None, &second).unwrap();
        store.save_mcp_server("server", None, &first).unwrap();
        let state = store.mcp_state().unwrap();
        assert_eq!(state.decisions.len(), 1);
        assert_eq!(state.decisions[0].decision, "approved");

        store
            .decide_mcp(
                "server",
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "rejected",
            )
            .unwrap();
        assert_eq!(store.mcp_state().unwrap().decisions.len(), 2);
        store
            .revoke_mcp("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap();
        assert_eq!(store.mcp_state().unwrap().decisions[0].decision, "rejected");
        std::fs::remove_dir_all(dir).ok();
    }
}
