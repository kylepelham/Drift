use super::Store;
use crate::id;
use crate::session::types::{Part, PartRow};
use rusqlite::{Connection, OptionalExtension, Row, params};

// Both name the partial index's condition so SQLite answers from `idx_part_image` instead of scanning every part.
const LAST_IMAGE_NUMBER: &str = "SELECT COALESCE(MAX(json_extract(json, '$.image')), 0) FROM part
     WHERE session_id = ?1 AND json_extract(json, '$.image') IS NOT NULL";
const IMAGE_NUMBERED: &str = "SELECT json FROM part
     WHERE session_id = ?1 AND json_extract(json, '$.image') IS NOT NULL AND json_extract(json, '$.image') = ?2
     ORDER BY id DESC LIMIT 1";
// Names the session/call index's condition, so only the session's tool calls are read; a diff is read only for a call without per-file records.
const EDIT_CALLS: &str = "SELECT COALESCE(json_extract(json, '$.input.path'), json_extract(json, '$.input.filePath')),
     json_extract(json, '$.metadata.files'), json_extract(json, '$.metadata.fileChanges'),
     CASE WHEN json_extract(json, '$.metadata.fileChanges') IS NULL THEN json_extract(json, '$.metadata.diff') END
     FROM part WHERE session_id = ?1 AND json_extract(json, '$.type') = 'tool_call'
     AND json_extract(json, '$.name') IN ('edit', 'write', 'apply_patch') AND json_extract(json, '$.status') = 'done'
     ORDER BY message_id, id";

/// What a finished file-tool call says it changed, as stored: arrays stay JSON text.
pub struct EditCall {
    pub path: Option<String>,
    pub files: Option<String>,
    pub file_changes: Option<String>,
    pub diff: Option<String>,
}

impl Store {
    /// Finds one call through its session/call-id index, without loading the transcript.
    pub fn call_part(&self, session_id: &str, call_id: &str) -> rusqlite::Result<Option<PartRow>> {
        self.read()
            .prepare_cached(
                "SELECT id, session_id, json, message_id, provider_signature FROM part
                 WHERE session_id = ?1 AND json_extract(json, '$.callId') = ?2
                 AND json_extract(json, '$.type') = 'tool_call' ORDER BY id LIMIT 1",
            )?
            .query_row(params![session_id, call_id], |row| {
                let message_id: String = row.get(3)?;
                map_part(row, &message_id)
            })
            .optional()
    }

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

    /// The highest number a pasted image of the session was given; 0 before the first.
    pub fn last_image_number(&self, session_id: &str) -> rusqlite::Result<u32> {
        self.read()
            .prepare_cached(LAST_IMAGE_NUMBER)?
            .query_row(params![session_id], |row| row.get(0))
    }

    /// The pasted image the session knows by `number`; the newest when a race gave two the same.
    pub fn image_numbered(&self, session_id: &str, number: u32) -> rusqlite::Result<Option<Part>> {
        let json: Option<String> = self
            .read()
            .prepare_cached(IMAGE_NUMBERED)?
            .query_row(params![session_id, number], |row| row.get(0))
            .optional()?;

        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// The session's finished file-tool calls, oldest first.
    pub fn edit_calls(&self, session_id: &str) -> rusqlite::Result<Vec<EditCall>> {
        let connection = self.read();
        let mut statement = connection.prepare_cached(EDIT_CALLS)?;
        let calls = statement.query_map(params![session_id], |row| {
            Ok(EditCall {
                path: row.get(0)?,
                files: row.get(1)?,
                file_changes: row.get(2)?,
                diff: row.get(3)?,
            })
        })?;

        calls.collect()
    }

    /// The session's messages with a tool call still pending or running.
    pub fn messages_with_open_calls(&self, session_id: &str) -> rusqlite::Result<std::collections::HashSet<String>> {
        let connection = self.read();
        let mut statement = connection.prepare_cached(
            "SELECT DISTINCT message_id FROM part WHERE session_id = ?1 AND json_extract(json, '$.type') = 'tool_call'
             AND json_extract(json, '$.status') IN ('pending', 'running')",
        )?;
        let open = statement.query_map(params![session_id], |row| row.get(0))?;

        open.collect()
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

#[cfg(test)]
mod tests {
    use crate::store::tests::store;
    use crate::store::tree::tests::plan;

    #[test]
    fn numbering_a_prompts_images_looks_up_the_session_in_its_index() {
        let store = store();

        let last = plan(&store, super::LAST_IMAGE_NUMBER, ["ses"]);
        let named = plan(&store, super::IMAGE_NUMBERED, rusqlite::params!["ses", 2]);

        assert!(
            last.iter().any(|step| step.contains("COVERING INDEX idx_part_image")),
            "{last:?}"
        );
        assert!(
            named.iter().any(|step| step.contains("INDEX idx_part_image")),
            "{named:?}"
        );
    }

    #[test]
    fn a_workers_edits_are_read_from_its_tool_calls_alone() {
        let calls = plan(&store(), super::EDIT_CALLS, ["ses"]);

        assert!(
            calls.iter().any(|step| step.contains("INDEX idx_part_call")),
            "{calls:?}"
        );
    }
}
