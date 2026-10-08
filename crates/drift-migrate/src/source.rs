//! Reads opencode's database read-only, from a consistent snapshot and in bounded pages.
//! No conversation is held whole in memory, regardless of its length.

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::path::Path;

/// Tool parts beyond this byte limit are streamed and reduced to their name, input and output prefix.
/// Other part kinds are read whole; oversized tool display copies are skipped.
const OVERSIZED_PART_BYTES: i64 = 8_000_000;
/// Maximum bytes retained from an oversized call's input or output.
const KEPT_BYTES: i64 = 64 * 1024;

pub(crate) struct Source {
    conn: Connection,
}

pub(crate) struct OcSession {
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

pub(crate) struct OcMessage {
    pub id: String,
    pub created: i64,
    pub data: String,
}

pub(crate) struct OcPart {
    pub id: String,
    pub data: String,
}

pub(crate) struct OcTodo {
    pub content: String,
    pub status: String,
    pub priority: String,
}

/// Fields retained from an oversized tool call.
/// The streaming reader skips all other fields, including display copies, without storing them.
#[derive(serde::Deserialize)]
struct BigCall {
    #[serde(rename = "type")]
    kind: Option<String>,
    tool: Option<String>,
    #[serde(rename = "callID")]
    call_id: Option<String>,
    #[serde(default)]
    state: BigState,
}

#[derive(Default, serde::Deserialize)]
struct BigState {
    status: Option<String>,
    title: Option<String>,
    input: Option<Value>,
    output: Option<String>,
    error: Option<String>,
    time: Option<Value>,
}

impl BigCall {
    fn kept(self) -> Value {
        let input = self
            .state
            .input
            .filter(|input| input.to_string().len() <= KEPT_BYTES as usize);

        json!({ "type": "tool", "tool": self.tool, "callID": self.call_id, "state": {
            "status": self.state.status, "title": self.state.title, "time": self.state.time,
            "input": input, "output": truncate(self.state.output), "error": truncate(self.state.error),
        } })
    }
}

fn truncate(text: Option<String>) -> Option<String> {
    text.map(|mut text| {
        let mut boundary = (KEPT_BYTES as usize).min(text.len());
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }

        text.truncate(boundary);
        text
    })
}

fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OcMessage> {
    Ok(OcMessage {
        id: row.get(0)?,
        created: row.get(1)?,
        data: row.get(2)?,
    })
}

impl Source {
    /// Opens read-only and holds one read transaction, keeping all rows consistent while opencode writes.
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        conn.execute_batch("BEGIN")?;
        conn.query_row("SELECT count(*) FROM session", [], |_| Ok(()))?;

        Ok(Self { conn })
    }

    /// Returns all conversations, most recently used first so the sidebar fills from the top.
    /// Subagents follow the conversations that could have started them.
    pub(crate) fn sessions(&self) -> rusqlite::Result<Vec<OcSession>> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.parent_id, s.directory, s.title, s.agent, s.model,
                    s.time_created, s.time_updated, s.time_archived IS NOT NULL, p.worktree
             FROM session s LEFT JOIN project p ON p.id = s.project_id
             ORDER BY s.parent_id IS NOT NULL, s.time_updated DESC, s.id",
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

    /// Returns up to limit messages after after's creation time and ID, in written order.
    /// Per-file diff summaries on user messages are left in the database rather than read.
    pub(crate) fn messages_after(
        &self,
        session_id: &str,
        after: Option<&OcMessage>,
        limit: usize,
    ) -> rusqlite::Result<Vec<OcMessage>> {
        let (created, id) = after.map_or((i64::MIN, ""), |message| (message.created, message.id.as_str()));
        let mut statement = self.conn.prepare_cached(
            "SELECT id, time_created, json_remove(data, '$.summary.diffs') FROM message
             WHERE session_id = ?1 AND (time_created > ?2 OR (time_created = ?2 AND id > ?3))
             ORDER BY time_created, id LIMIT ?4",
        )?;
        let rows = statement.query_map(params![session_id, created, id, limit as i64], message_row)?;

        rows.collect()
    }

    /// Returns the conversation's newest limit messages, newest first.
    pub(crate) fn newest_messages(&self, session_id: &str, limit: usize) -> rusqlite::Result<Vec<OcMessage>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT id, time_created, '' FROM message WHERE session_id = ?1
             ORDER BY time_created DESC, id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![session_id, limit as i64], message_row)?;

        rows.collect()
    }

    /// Returns a message's parts in order, reducing tool calls larger than OVERSIZED_PART_BYTES.
    /// Oversized tool calls are streamed from disk rather than held whole in memory.
    pub(crate) fn parts(&self, message_id: &str) -> rusqlite::Result<Vec<OcPart>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT id, rowid, CASE WHEN octet_length(data) <= ?2 THEN data END FROM part
             WHERE message_id = ?1 ORDER BY id",
        )?;
        let rows: Vec<(String, i64, Option<String>)> = statement
            .query_map(params![message_id, OVERSIZED_PART_BYTES], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;

        rows.into_iter()
            .map(|(id, rowid, data)| {
                let data = match data {
                    Some(data) => data,
                    None => self.oversized(rowid)?,
                };
                Ok(OcPart { id, data })
            })
            .collect()
    }

    /// Reduces a tool call to its retained fields; any other oversized part is read whole.
    fn oversized(&self, rowid: i64) -> rusqlite::Result<String> {
        let stream = self.conn.blob_open("main", "part", "data", rowid, true)?;

        match serde_json::from_reader::<_, BigCall>(std::io::BufReader::new(stream)) {
            Ok(call) if call.kind.as_deref() == Some("tool") => Ok(call.kept().to_string()),
            _ => self
                .conn
                .prepare_cached("SELECT data FROM part WHERE rowid = ?1")?
                .query_row([rowid], |row| row.get(0)),
        }
    }

    /// A part's stored text when it is within [`OVERSIZED_PART_BYTES`].
    pub(crate) fn small_part(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .prepare_cached("SELECT data FROM part WHERE id = ?1 AND octet_length(data) <= ?2")?
            .query_row(params![id, OVERSIZED_PART_BYTES], |row| row.get(0))
            .optional()
    }

    /// Returns a message's part IDs without reading their text.
    pub(crate) fn part_ids(&self, message_id: &str) -> rusqlite::Result<Vec<String>> {
        self.conn
            .prepare_cached("SELECT id FROM part WHERE message_id = ?1 ORDER BY id")?
            .query_map([message_id], |row| row.get(0))?
            .collect()
    }

    /// Returns conversations containing queued prompts that opencode never ran.
    /// Databases predating the queue table have no pending prompts.
    pub(crate) fn pending_inputs(&self) -> std::collections::HashSet<String> {
        let ids = self
            .conn
            .prepare("SELECT DISTINCT session_id FROM session_input WHERE promoted_seq IS NULL")
            .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect());

        ids.unwrap_or_default()
    }

    pub(crate) fn todos(&self, session_id: &str) -> rusqlite::Result<Vec<OcTodo>> {
        self.conn
            .prepare_cached("SELECT content, status, priority FROM todo WHERE session_id = ?1 ORDER BY position")?
            .query_map([session_id], |row| {
                Ok(OcTodo {
                    content: row.get(0)?,
                    status: row.get(1)?,
                    priority: row.get(2)?,
                })
            })?
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_text_is_cut_at_a_utf8_boundary_and_missing_text_stays_missing() {
        let prefix = "x".repeat(KEPT_BYTES as usize - 1);
        let text = format!("{prefix}é");

        assert_eq!(truncate(Some(text)), Some(prefix));
        assert_eq!(truncate(None), None);
        assert_eq!(truncate(Some("short".into())), Some("short".into()));
    }
}
