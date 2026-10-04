//! opencode's database, opened read-only: the rows of one conversation as opencode stored them.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

pub struct Source {
    conn: Connection,
}

pub struct OcSession {
    pub id: String,
    pub parent_id: Option<String>,
    pub directory: String,
    /// The repository root opencode filed it under; the conversation may have run in a directory inside it.
    pub worktree: Option<String>,
    pub title: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub archived: bool,
}

pub struct OcMessage {
    pub id: String,
    pub created: i64,
    pub data: String,
    pub parts: Vec<OcPart>,
}

pub struct OcPart {
    pub id: String,
    pub data: String,
}

pub struct OcTodo {
    pub content: String,
    pub status: String,
    pub priority: String,
}

impl Source {
    /// Opens read-only and holds one read transaction, so every row comes from the same moment even while opencode writes.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("BEGIN")?;
        conn.query_row("SELECT count(*) FROM session", [], |_| Ok(()))?;
        Ok(Self { conn })
    }

    /// Every conversation, parents before the subagents they started.
    pub fn sessions(&self) -> rusqlite::Result<Vec<OcSession>> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.parent_id, s.directory, s.title, s.agent, s.model, s.time_created, s.time_updated, s.time_archived IS NOT NULL, p.worktree
             FROM session s LEFT JOIN project p ON p.id = s.project_id ORDER BY s.parent_id IS NOT NULL, s.time_created, s.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(OcSession {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                directory: row.get(2)?,
                worktree: row.get(9)?,
                title: row.get(3)?,
                agent: row.get(4)?,
                model: row.get(5)?,
                created: row.get(6)?,
                updated: row.get(7)?,
                archived: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// The conversation's messages in the order they were written, each with its parts in order.
    pub fn messages(&self, session_id: &str) -> rusqlite::Result<Vec<OcMessage>> {
        let mut messages: Vec<OcMessage> = self
            .conn
            .prepare_cached("SELECT id, time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")?
            .query_map([session_id], |row| Ok(OcMessage { id: row.get(0)?, created: row.get(1)?, data: row.get(2)?, parts: Vec::new() }))?
            .collect::<rusqlite::Result<_>>()?;
        let index: std::collections::HashMap<String, usize> = messages.iter().enumerate().map(|(at, message)| (message.id.clone(), at)).collect();
        let mut statement = self.conn.prepare_cached("SELECT message_id, id, data FROM part WHERE session_id = ?1 ORDER BY message_id, id")?;
        let parts = statement.query_map([session_id], |row| Ok((row.get::<_, String>(0)?, OcPart { id: row.get(1)?, data: row.get(2)? })))?;
        for part in parts {
            let (message_id, part) = part?;
            if let Some(&at) = index.get(&message_id) {
                messages[at].parts.push(part);
            }
        }
        Ok(messages)
    }

    pub fn todos(&self, session_id: &str) -> rusqlite::Result<Vec<OcTodo>> {
        self.conn
            .prepare_cached("SELECT content, status, priority FROM todo WHERE session_id = ?1 ORDER BY position")?
            .query_map([session_id], |row| Ok(OcTodo { content: row.get(0)?, status: row.get(1)?, priority: row.get(2)? }))?
            .collect()
    }
}
