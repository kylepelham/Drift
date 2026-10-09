use super::Store;
use crate::id;
use crate::session::types::{Part, PartRow};
use rusqlite::{Connection, Row, params};

impl Store {
    /// A tool call whose id is empty or already used in the session gets an engine id: tasks, spilled
    /// output and recovery all find a call by its id.
    pub fn add_part(&self, message_id: &str, session_id: &str, mut part: Part) -> rusqlite::Result<PartRow> {
        let connection = self.lock();
        if let Part::ToolCall { call_id, .. } = &mut part {
            let taken = call_id.is_empty() || connection
                .prepare_cached("SELECT 1 FROM part WHERE session_id = ?1 AND json_extract(json, '$.callId') = ?2 AND json_extract(json, '$.type') = 'tool_call'")?
                .exists(params![session_id, call_id.as_str()])?;
            if taken {
                *call_id = id::new("call");
            }
        }

        insert_part(&connection, message_id, session_id, part)
    }

    /// Saves the part; one that was streaming is closed, so reads take it from here.
    pub fn save_part(&self, row: &PartRow) -> rusqlite::Result<()> {
        save_part_in(&self.lock(), row)?;
        self.streaming.lock().unwrap().remove(&row.id);

        Ok(())
    }

    /// The part as far as it has streamed, for every read until it closes; not written to disk.
    pub fn stream_part(&self, row: &PartRow) {
        self.streaming.lock().unwrap().insert(row.id.clone(), row.clone());
    }

    /// Writes a still-streaming part as it stands, so a crash loses at most what came since.
    pub fn checkpoint_part(&self, row: &PartRow) -> rusqlite::Result<()> {
        save_part_in(&self.lock(), row)
    }
}

pub(super) fn save_part_in(connection: &Connection, row: &PartRow) -> rusqlite::Result<()> {
    connection
        .prepare_cached("UPDATE part SET json = ?2, provider_signature = ?3 WHERE id = ?1")?
        .execute(params![row.id, row.part.stored(), row.provider_signature])?;

    Ok(())
}

pub(super) fn insert_part(
    connection: &Connection,
    message_id: &str,
    session_id: &str,
    part: Part,
) -> rusqlite::Result<PartRow> {
    let row = PartRow {
        id: id::new("prt"),
        message_id: message_id.into(),
        session_id: session_id.into(),
        provider_signature: None,
        part,
    };

    connection
        .prepare_cached("INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)")?
        .execute(params![row.id, row.message_id, row.session_id, row.part.stored()])?;

    Ok(row)
}

pub(super) fn map_part(row: &Row, message_id: &str) -> rusqlite::Result<PartRow> {
    Ok(PartRow {
        id: row.get(0)?,
        message_id: message_id.into(),
        session_id: row.get(1)?,
        provider_signature: row.get(4)?,
        part: Part::from_stored(&row.get::<_, String>(2)?),
    })
}
