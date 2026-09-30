//! Whole-session operations: copying a transcript into a fork, moving a session with its subagents.

use rusqlite::{params, Connection};

use super::sessions::{insert_session, session_from, transaction, NewSession};
use super::Store;
use crate::id;
use crate::session::types::Session;

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
            for message in &messages {
                copy_message(conn, message, &session.id)?;
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

fn copy_message(conn: &Connection, source: &str, session_id: &str) -> rusqlite::Result<()> {
    let message_id = id::new("msg");
    conn.prepare_cached(
        "INSERT INTO message(id, session_id, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at)
         SELECT ?1, ?2, role, status, model_provider, model_id, usage_json, cost, error, created_at, finished_at FROM message WHERE id = ?3",
    )?
    .execute(params![message_id, session_id, source])?;
    let parts: Vec<String> = conn
        .prepare_cached("SELECT id FROM part WHERE message_id = ?1 ORDER BY id")?
        .query_map([source], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut insert = conn.prepare_cached("INSERT INTO part(id, message_id, session_id, json) SELECT ?1, ?2, ?3, json FROM part WHERE id = ?4")?;
    for part in parts {
        insert.execute(params![id::new("prt"), message_id, session_id, part])?;
    }
    Ok(())
}
