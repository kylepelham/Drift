//! Each session's instruction baseline: the hash of every system prompt source as the session first saw
//! it, and as it last told the model. The texts are blobs, stored once whoever names them.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::Digest;

use super::Store;
use super::sessions::transaction;
use crate::id;

/// One source a session knows: its place in the baseline and the hashes of what the model was told.
#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub key: String,
    pub position: usize,
    /// In the frozen system prompt; `None` for a source that arrived later as an update.
    pub baseline: Option<String>,
    /// What the model last heard; `None` once the source went away.
    pub current: Option<String>,
    /// The baseline this row belongs to, the same for every row of a session.
    pub generation: String,
}

/// A source's key and text, as the baseline or an update records it.
pub struct Value<'a> {
    pub key: &'a str,
    pub text: &'a str,
}

fn put_value(conn: &Connection, text: &str) -> rusqlite::Result<String> {
    let hash = crate::hex_bytes(&sha2::Sha256::digest(text.as_bytes()));
    conn.prepare_cached("INSERT OR IGNORE INTO blob(hash, data, created_at) VALUES(?1, ?2, ?3)")?
        .execute(params![hash, text.as_bytes(), id::now_ms()])?;

    Ok(hash)
}

/// A session's instruction updates in order, found through its messages rather than by scanning every part.
const INSTRUCTION_UPDATES: &str = "SELECT part.json FROM message JOIN part ON part.message_id = message.id
     WHERE message.session_id = ?1 AND json_extract(part.json, '$.type') = 'instruction_update'
     ORDER BY message.id, part.id";

impl Store {
    /// What the session knows of its sources, in baseline order; empty before its first request.
    pub fn instructions(&self, session_id: &str) -> rusqlite::Result<Vec<Instruction>> {
        self.read()
            .prepare_cached(
                "SELECT key, position, baseline, current, generation FROM instruction WHERE session_id = ?1 ORDER BY position",
            )?
            .query_map([session_id], |row| {
                Ok(Instruction {
                    key: row.get(0)?,
                    position: row.get::<_, i64>(1)? as usize,
                    baseline: row.get(2)?,
                    current: row.get(3)?,
                    generation: row.get(4)?,
                })
            })?
            .collect()
    }

    /// Freezes `values` as the session's baseline, replacing whatever it had; returns its generation.
    pub fn set_instruction_baseline(&self, session_id: &str, values: &[Value]) -> rusqlite::Result<String> {
        let generation = id::new("base");
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("DELETE FROM instruction WHERE session_id = ?1")?
                .execute([session_id])?;
            for (position, value) in values.iter().enumerate() {
                let hash = put_value(conn, value.text)?;
                conn.prepare_cached(
                    "INSERT INTO instruction(session_id, key, position, baseline, current, generation) VALUES(?1, ?2, ?3, ?4, ?4, ?5)",
                )?
                .execute(params![session_id, value.key, position as i64, hash, generation])?;
            }
            Ok(())
        })?;

        Ok(generation)
    }

    /// Records what the model was just told: a new text for each value, and `gone` sources as removed.
    pub fn update_instructions(&self, session_id: &str, values: &[Value], gone: &[&str]) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            for value in values {
                let hash = put_value(conn, value.text)?;
                conn.prepare_cached(
                    "INSERT INTO instruction(session_id, key, position, baseline, current, generation)
                     VALUES(?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM instruction WHERE session_id = ?1), NULL, ?3,
                            (SELECT generation FROM instruction WHERE session_id = ?1 LIMIT 1))
                     ON CONFLICT(session_id, key) DO UPDATE SET current = excluded.current",
                )?
                .execute(params![session_id, value.key, hash])?;
            }
            for key in gone {
                conn.prepare_cached("UPDATE instruction SET current = NULL WHERE session_id = ?1 AND key = ?2")?
                    .execute(params![session_id, key])?;
            }
            Ok(())
        })
    }

    /// Adds `key` to the session's current baseline, as heard from the start.
    pub fn add_to_instruction_baseline(&self, session_id: &str, key: &str, text: &str) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            let hash = put_value(conn, text)?;
            conn.prepare_cached(
                "INSERT OR REPLACE INTO instruction(session_id, key, position, baseline, current, generation)
                 SELECT ?1, ?2, COALESCE(MAX(position), -1) + 1, ?3, ?3, generation FROM instruction WHERE session_id = ?1",
            )?
            .execute(params![session_id, key, hash])?;
            Ok(())
        })
    }

    /// The text a hash names, as `put_value` kept it.
    pub fn instruction_text(&self, hash: &str) -> rusqlite::Result<Option<String>> {
        Ok(self
            .blob(hash)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Forgets the baseline, so the session's next request freezes a new one.
    pub fn clear_instructions(&self, session_id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("DELETE FROM instruction WHERE session_id = ?1")?
            .execute([session_id])?;

        Ok(())
    }

    /// Gives `fork_id` the baseline `source_id` has, its sources as the copied history last told them:
    /// the baseline, then each `instruction_update` the fork copied, in order. Its prefix then matches its
    /// source's byte for byte, and a change the source heard later is still news to the fork.
    pub fn fork_instructions(&self, source_id: &str, fork_id: &str) -> rusqlite::Result<()> {
        transaction(&self.lock(), |conn| {
            conn.prepare_cached("DELETE FROM instruction WHERE session_id = ?2")?
                .execute([source_id, fork_id])?;
            conn.prepare_cached(
                "INSERT INTO instruction(session_id, key, position, baseline, current, generation)
                 SELECT ?2, key, position, baseline, baseline, generation FROM instruction WHERE session_id = ?1 AND baseline IS NOT NULL",
            )?
            .execute([source_id, fork_id])?;

            let updates: Vec<String> = conn
                .prepare_cached(INSTRUCTION_UPDATES)?
                .query_map([fork_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for json in updates {
                replay(conn, fork_id, &json)?;
            }
            Ok(())
        })
    }
}

/// Applies one saved update to `session_id`'s sources, as `update_instructions` did when it was sent.
fn replay(conn: &Connection, session_id: &str, json: &str) -> rusqlite::Result<()> {
    let Ok(crate::session::types::Part::InstructionUpdate { generation, changes }) = serde_json::from_str(json) else {
        return Ok(());
    };
    let current: Option<String> = conn
        .prepare_cached("SELECT generation FROM instruction WHERE session_id = ?1 LIMIT 1")?
        .query_row([session_id], |row| row.get(0))
        .optional()?;
    if current.as_deref() != Some(generation.as_str()) {
        return Ok(());
    }
    for change in changes {
        let current = change.text.as_deref().map(|text| put_value(conn, text)).transpose()?;
        conn.prepare_cached(
            "INSERT INTO instruction(session_id, key, position, baseline, current, generation)
             VALUES(?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM instruction WHERE session_id = ?1), NULL, ?3,
                    (SELECT generation FROM instruction WHERE session_id = ?1 LIMIT 1))
             ON CONFLICT(session_id, key) DO UPDATE SET current = excluded.current",
        )?
        .execute(params![session_id, change.key, current])?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::store::tests::store;
    use crate::store::tree::tests::plan;

    #[test]
    fn a_fork_replays_its_instruction_updates_without_scanning_every_part() {
        let plan = plan(&store(), super::INSTRUCTION_UPDATES, ["ses"]);

        assert!(plan.iter().all(|step| !step.starts_with("SCAN")), "{plan:?}");
    }
}
