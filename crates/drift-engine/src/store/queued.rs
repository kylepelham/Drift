//! Prompts the user queued for after the running turn, kept outside the transcript until one starts a turn.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Store;
use crate::session::turn::Prompt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedPrompt {
    pub id: String,
    pub session_id: String,
    pub prompt: Prompt,
    pub created_at: i64,
    /// Why it could not start a turn; the queue waits until the user edits, sends or discards it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

type Row = (String, String, String, i64, Option<String>);

fn queued((id, session_id, prompt, created_at, error): Row) -> QueuedPrompt {
    QueuedPrompt {
        id,
        session_id,
        prompt: serde_json::from_str(&prompt).unwrap_or_default(),
        created_at,
        error,
    }
}

const COLUMNS: &str = "id, session_id, prompt, created_at, error";

impl Store {
    pub fn queue_prompt(&self, id: &str, session_id: &str, prompt: &Prompt) -> rusqlite::Result<()> {
        let json = serde_json::to_string(prompt).unwrap_or_default();
        self.lock()
            .prepare_cached(
                "INSERT INTO queued_prompt(id, session_id, position, prompt, created_at)
                 VALUES(?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM queued_prompt WHERE session_id = ?2), ?3, ?4)",
            )?
            .execute(params![id, session_id, json, crate::id::now_ms()])?;

        Ok(())
    }

    /// The session's queue, first to start first.
    pub fn queued_prompts(&self, session_id: &str) -> rusqlite::Result<Vec<QueuedPrompt>> {
        let sql = format!("SELECT {COLUMNS} FROM queued_prompt WHERE session_id = ?1 ORDER BY position");
        self.lock()
            .prepare_cached(&sql)?
            .query_map([session_id], |row| {
                Ok(queued((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                )))
            })?
            .collect()
    }

    pub fn queued_prompt(&self, id: &str) -> rusqlite::Result<Option<QueuedPrompt>> {
        let sql = format!("SELECT {COLUMNS} FROM queued_prompt WHERE id = ?1");
        self.lock()
            .prepare_cached(&sql)?
            .query_row([id], |row| {
                Ok(queued((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                )))
            })
            .optional()
    }

    /// Replaces a queued prompt's text and clears any error; false when it is no longer queued.
    pub fn edit_queued_prompt(&self, id: &str, prompt: &Prompt) -> rusqlite::Result<bool> {
        let json = serde_json::to_string(prompt).unwrap_or_default();
        let changed = self
            .lock()
            .prepare_cached("UPDATE queued_prompt SET prompt = ?2, error = NULL WHERE id = ?1")?
            .execute(params![id, json])?;

        Ok(changed > 0)
    }

    pub fn fail_queued_prompt(&self, id: &str, error: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("UPDATE queued_prompt SET error = ?2 WHERE id = ?1")?
            .execute(params![id, error])?;

        Ok(())
    }

    /// Takes a prompt off the queue; false when it was already gone.
    pub fn unqueue_prompt(&self, id: &str) -> rusqlite::Result<bool> {
        let removed = self
            .lock()
            .prepare_cached("DELETE FROM queued_prompt WHERE id = ?1")?
            .execute([id])?;

        Ok(removed > 0)
    }
}
