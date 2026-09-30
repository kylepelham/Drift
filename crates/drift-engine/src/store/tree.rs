//! Whole-session operations: copying a transcript into a fork, moving a session with its subagents.

use std::collections::HashMap;

use rusqlite::types::Type;
use rusqlite::{params, Connection};

use super::sessions::{insert_session, session_from, transaction, NewSession};
use super::Store;
use crate::id;
use crate::session::types::{Part, Session};

impl Store {
    /// A new session holding copies of the source's finished messages up to and including `through`.
    pub fn fork_session(&self, source_id: &str, new: NewSession, through: &str) -> rusqlite::Result<Session> {
        let session = session_from(new, None);
        transaction(&self.lock(), |conn| {
            insert_session(conn, &session)?;
            let messages: Vec<String> = conn
                .prepare_cached("SELECT id FROM message WHERE session_id = ?1 AND id <= ?2 AND status != 'streaming' ORDER BY id")?
                .query_map(params![source_id, through], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut copies = HashMap::new();
            for message in &messages {
                copy_message(conn, message, &session.id, &mut copies)?;
            }
            Ok(())
        })?;
        Ok(session)
    }

    /// The session and its subagents, at any depth. Branches are independent and stay behind.
    pub fn session_tree(&self, id: &str) -> rusqlite::Result<Vec<String>> {
        self.lock()
            .prepare_cached(
                "WITH RECURSIVE tree(id) AS (
                     SELECT id FROM session WHERE id = ?1
                     UNION SELECT s.id FROM session s JOIN tree ON s.parent_id = tree.id WHERE s.visibility = 'hidden'
                 ) SELECT id FROM tree",
            )?
            .query_map([id], |row| row.get(0))?
            .collect()
    }

    /// Every shadow blob a recorded change in the workspace refers to, archived sessions included:
    /// what undo and redo could still need.
    pub fn recorded_blobs(&self, workspace_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut statement = conn.prepare_cached(
            "SELECT p.json FROM part p JOIN session s ON s.id = p.session_id WHERE s.workspace_id = ?1 AND p.json LIKE '%\"changes\"%'",
        )?;
        let rows = statement.query_map([workspace_id], |row| row.get::<_, String>(0))?;
        let mut blobs = std::collections::BTreeSet::new();
        for json in rows {
            let Ok(Part::ToolCall { metadata: Some(metadata), .. }) = serde_json::from_str::<Part>(&json?) else { continue };
            let Some(changes) = metadata.get("changes").and_then(|c| c.as_array()) else { continue };
            for change in changes {
                blobs.extend(["before", "after"].iter().filter_map(|side| change[side].as_str().map(str::to_string)));
            }
        }
        Ok(blobs.into_iter().collect())
    }

    pub fn move_sessions(&self, ids: &[String], workspace_id: &str) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            let mut update = conn.prepare_cached("UPDATE session SET workspace_id = ?2, updated_at = ?3 WHERE id = ?1")?;
            let now = id::now_ms();
            for session in ids {
                update.execute(params![session, workspace_id, now])?;
            }
            Ok(())
        })
    }
}

/// `copies` maps source message ids to their copies, so a compaction boundary keeps pointing at its own tail.
fn copy_message(conn: &Connection, source: &str, session_id: &str, copies: &mut HashMap<String, String>) -> rusqlite::Result<()> {
    let message_id = id::new("msg");
    conn.prepare_cached(
        "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary)
         SELECT ?1, ?2, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at, summary FROM message WHERE id = ?3",
    )?
    .execute(params![message_id, session_id, source])?;
    copies.insert(source.to_string(), message_id.clone());
    let parts: Vec<Part> = conn
        .prepare_cached("SELECT json FROM part WHERE message_id = ?1 ORDER BY id")?
        .query_map([source], |row| row.get::<_, String>(0))?
        .map(|json| json.and_then(|json| serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e)))))
        .collect::<rusqlite::Result<_>>()?;
    let mut insert = conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?;
    for mut part in parts {
        if let Part::Compaction { tail_from, .. } = &mut part {
            *tail_from = tail_from.as_ref().and_then(|tail| copies.get(tail).cloned());
        }
        insert.execute(params![id::new("prt"), message_id, session_id, serde_json::to_string(&part).unwrap()])?;
    }
    Ok(())
}
