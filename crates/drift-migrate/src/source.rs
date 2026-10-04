//! opencode's database, opened read-only and read a page at a time, so no conversation is ever held
//! whole in memory however long it grew.

use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};

/// A stored part past this is cut down inside SQLite: a tool call keeps its name, input and the start
/// of its output; anything else is read whole. Eight parts in a 19 GB database were over it, the
/// largest 663 MB of patch display copies.
const OVERSIZED_PART_BYTES: i64 = 8_000_000;
/// A cut-down call keeps at most this much of its input and its output.
const KEPT_BYTES: i64 = 64 * 1024;

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
}

pub struct OcPart {
    pub id: String,
    pub data: String,
}

/// The fields kept from an oversized tool call; everything else (its display copies) is skipped while
/// reading, never stored.
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
        let cut = |text: Option<String>| text.map(|mut text| {
            let mut at = (KEPT_BYTES as usize).min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            text.truncate(at);
            text
        });
        let input = self.state.input.filter(|input| input.to_string().len() <= KEPT_BYTES as usize);
        json!({ "type": "tool", "tool": self.tool, "callID": self.call_id, "state": {
            "status": self.state.status, "title": self.state.title, "time": self.state.time,
            "input": input, "output": cut(self.state.output), "error": cut(self.state.error),
        } })
    }
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

    /// Up to `limit` messages after `after` (their creation time and id), in written order. A user
    /// message's per-file diff summary is left in the database: nothing here reads it.
    pub fn messages_after(&self, session_id: &str, after: Option<&OcMessage>, limit: usize) -> rusqlite::Result<Vec<OcMessage>> {
        let (created, id) = after.map_or((i64::MIN, ""), |message| (message.created, message.id.as_str()));
        self.conn
            .prepare_cached(
                "SELECT id, time_created, json_remove(data, '$.summary.diffs') FROM message
                 WHERE session_id = ?1 AND (time_created > ?2 OR (time_created = ?2 AND id > ?3)) ORDER BY time_created, id LIMIT ?4",
            )?
            .query_map(params![session_id, created, id, limit as i64], |row| Ok(OcMessage { id: row.get(0)?, created: row.get(1)?, data: row.get(2)? }))?
            .collect()
    }

    /// The conversation's newest `limit` messages, newest first.
    pub fn newest_messages(&self, session_id: &str, limit: usize) -> rusqlite::Result<Vec<OcMessage>> {
        self.conn
            .prepare_cached("SELECT id, time_created, '' FROM message WHERE session_id = ?1 ORDER BY time_created DESC, id DESC LIMIT ?2")?
            .query_map(params![session_id, limit as i64], |row| Ok(OcMessage { id: row.get(0)?, created: row.get(1)?, data: row.get(2)? }))?
            .collect()
    }

    /// A message's parts in order; a tool call stored past [`OVERSIZED_PART_BYTES`] arrives cut down,
    /// read off disk as a stream so it is never held whole.
    pub fn parts(&self, message_id: &str) -> rusqlite::Result<Vec<OcPart>> {
        let rows: Vec<(String, i64, Option<String>)> = self
            .conn
            .prepare_cached("SELECT id, rowid, CASE WHEN octet_length(data) <= ?2 THEN data END FROM part WHERE message_id = ?1 ORDER BY id")?
            .query_map(params![message_id, OVERSIZED_PART_BYTES], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
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

    /// A tool call cut down to what is kept; any other part, rare at this size, read whole.
    fn oversized(&self, rowid: i64) -> rusqlite::Result<String> {
        let stream = self.conn.blob_open("main", "part", "data", rowid, true)?;
        match serde_json::from_reader::<_, BigCall>(std::io::BufReader::new(stream)) {
            Ok(call) if call.kind.as_deref() == Some("tool") => Ok(call.kept().to_string()),
            _ => self.conn.prepare_cached("SELECT data FROM part WHERE rowid = ?1")?.query_row([rowid], |row| row.get(0)),
        }
    }

    /// A part's stored text when it is within [`OVERSIZED_PART_BYTES`].
    pub fn small_part(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.conn.prepare_cached("SELECT data FROM part WHERE id = ?1 AND octet_length(data) <= ?2")?.query_row(params![id, OVERSIZED_PART_BYTES], |row| row.get(0)).optional()
    }

    /// The ids of a message's parts, without reading their text.
    pub fn part_ids(&self, message_id: &str) -> rusqlite::Result<Vec<String>> {
        self.conn.prepare_cached("SELECT id FROM part WHERE message_id = ?1 ORDER BY id")?.query_map([message_id], |row| row.get(0))?.collect()
    }

    pub fn todos(&self, session_id: &str) -> rusqlite::Result<Vec<OcTodo>> {
        self.conn
            .prepare_cached("SELECT content, status, priority FROM todo WHERE session_id = ?1 ORDER BY position")?
            .query_map([session_id], |row| Ok(OcTodo { content: row.get(0)?, status: row.get(1)?, priority: row.get(2)? }))?
            .collect()
    }
}
