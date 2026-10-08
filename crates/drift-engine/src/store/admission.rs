use super::Store;
use super::messages::{NewMessage, insert_message};
use super::parts::insert_part;
use super::sessions::{session_in, transaction};
use crate::id;
use crate::session::types::{Message, ModelRef, Part, PartRow, Revert, Role, Session};
use rusqlite::{Connection, OptionalExtension, params};

#[cfg(test)]
#[path = "tests/admission.rs"]
mod tests;

pub struct Admission<'a> {
    pub pick: Pick<'a>,
    pub parts: Vec<Part>,
    pub submission: Option<(&'a str, &'a str)>,
    pub handover: Handover<'a>,
}

/// How an admission ended. Only `New` wrote anything.
pub enum Admit {
    New(Box<Admitted>),
    /// The same submission id with the same prompt already landed as this message.
    Replayed {
        message_id: String,
    },
    /// The submission id was used for a different prompt or session.
    Conflict,
    /// The worker result this prompt carries was already handed over.
    Delivered,
}

/// Worker results a prompt carries into its session.
#[derive(Default)]
pub struct Handover<'a> {
    /// The prompt is this result's delivery; nothing lands unless the result is still owed.
    pub delivery: Option<&'a str>,
    /// Held results riding along, ahead of the prompt's own parts; each goes in only if still owed.
    pub held: Vec<(String, Part)>,
}

/// What a prompt runs on, written to its session as it lands: the model, and the variant and agent when the prompt chose them.
#[derive(Clone, Copy)]
pub struct Pick<'a> {
    pub model: &'a ModelRef,
    /// `None` keeps the session's; `Some(None)` clears it.
    pub variant: Option<Option<&'a str>>,
    pub agent: Option<&'a str>,
    /// False for a command's own agent or model: they run that turn only and the session keeps its choices.
    pub sticky: bool,
}

pub struct Admitted {
    pub message: Message,
    pub parts: Vec<PartRow>,
    pub session: Session,
    /// Messages an undo had hidden, now deleted because a new prompt went ahead from before them.
    pub discarded: Vec<String>,
}

pub struct Submission {
    pub session_id: String,
    pub message_id: String,
    pub payload_hash: String,
}

impl<'a> Pick<'a> {
    pub fn model(model: &'a ModelRef) -> Self {
        Self {
            model,
            variant: None,
            agent: None,
            sticky: true,
        }
    }
}

impl Store {
    /// Records a user prompt as one unit: message, parts and the session's model, or nothing at all.
    /// A prompt sent while undone commits the undo: the hidden messages go, in the same write.
    pub fn admit_prompt(
        &self,
        session_id: &str,
        model: &ModelRef,
        parts: Vec<Part>,
        submission: Option<(&str, &str)>,
    ) -> rusqlite::Result<Admitted> {
        let admission = Admission {
            pick: Pick::model(model),
            parts,
            submission,
            handover: Handover::default(),
        };

        match self.admit_delivering(session_id, admission)? {
            Admit::New(admitted) => Ok(*admitted),
            _ => Err(rusqlite::Error::QueryReturnedNoRows),
        }
    }

    /// [`Self::admit_prompt`] that also hands worker results over and settles a reused submission id, all in one write.
    pub fn admit_delivering(&self, session_id: &str, admission: Admission<'_>) -> rusqlite::Result<Admit> {
        let Admission {
            pick,
            parts,
            submission,
            handover,
        } = admission;
        let connection = self.lock();
        let Handover { delivery, held } = handover;

        transaction(&connection, |connection| {
            if let Some((id, hash)) = submission
                && let Some(earlier) = submission_in(connection, id)?
            {
                let same = earlier.session_id == session_id && earlier.payload_hash == hash;
                return Ok(if same {
                    Admit::Replayed {
                        message_id: earlier.message_id,
                    }
                } else {
                    Admit::Conflict
                });
            }
            if let Some(task_id) = delivery
                && !super::tasks::acknowledge(connection, task_id, session_id)?
            {
                return Ok(Admit::Delivered);
            }

            let carried = held_parts(connection, session_id, held)?;
            let parts = carried.into_iter().chain(parts).collect();
            admit_in(connection, session_id, pick, parts, submission).map(|admitted| Admit::New(Box::new(admitted)))
        })
    }

    pub fn submission(&self, id: &str) -> rusqlite::Result<Option<Submission>> {
        submission_in(&self.lock(), id)
    }
}

fn held_parts(connection: &Connection, session_id: &str, held: Vec<(String, Part)>) -> rusqlite::Result<Vec<Part>> {
    let mut carried = Vec::new();
    for (task_id, part) in held {
        if super::tasks::acknowledge(connection, &task_id, session_id)? {
            carried.push(part);
        }
    }

    Ok(carried)
}

fn admit_in(
    connection: &Connection,
    session_id: &str,
    pick: Pick,
    parts: Vec<Part>,
    submission: Option<(&str, &str)>,
) -> rusqlite::Result<Admitted> {
    let Pick {
        model,
        variant,
        agent,
        sticky,
    } = pick;
    let discarded = discard_reverted(connection, session_id)?;
    if sticky {
        connection
            .prepare_cached(
                "UPDATE session SET model_provider = ?2, model_id = ?3, updated_at = ?4,
                variant = CASE WHEN ?5 THEN ?6 ELSE variant END, agent = COALESCE(?7, agent) WHERE id = ?1",
            )?
            .execute(params![
                session_id,
                model.provider,
                model.model,
                id::now_ms(),
                variant.is_some(),
                variant.flatten(),
                agent
            ])?;
    } else {
        connection
            .prepare_cached("UPDATE session SET updated_at = ?2 WHERE id = ?1")?
            .execute(params![session_id, id::now_ms()])?;
    }

    let message = insert_message(connection, NewMessage::new(session_id, Role::User, Some(model)))?;
    if let Some((id, hash)) = submission {
        connection.prepare_cached("INSERT INTO submission(id, session_id, message_id, payload_hash, created_at) VALUES(?1, ?2, ?3, ?4, ?5)")?
            .execute(params![id, session_id, message.id, hash, id::now_ms()])?;
    }
    let rows = parts
        .into_iter()
        .map(|part| insert_part(connection, &message.id, session_id, part))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let session = session_in(connection, session_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;

    Ok(Admitted {
        message,
        parts: rows,
        session,
        discarded,
    })
}

fn discard_reverted(connection: &Connection, session_id: &str) -> rusqlite::Result<Vec<String>> {
    let revert: Option<Revert> = connection
        .prepare_cached("SELECT revert_json FROM session WHERE id = ?1")?
        .query_row([session_id], |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok());
    let Some(revert) = revert else { return Ok(Vec::new()) };

    let ids = connection
        .prepare_cached("SELECT id FROM message WHERE session_id = ?1 AND id >= ?2 ORDER BY id")?
        .query_map(params![session_id, revert.message_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    connection
        .prepare_cached("DELETE FROM submission WHERE session_id = ?1 AND message_id >= ?2")?
        .execute(params![session_id, revert.message_id])?;
    connection
        .prepare_cached("DELETE FROM message WHERE session_id = ?1 AND id >= ?2")?
        .execute(params![session_id, revert.message_id])?;
    connection
        .prepare_cached("UPDATE session SET revert_json = NULL WHERE id = ?1")?
        .execute([session_id])?;

    Ok(ids)
}

fn submission_in(connection: &Connection, id: &str) -> rusqlite::Result<Option<Submission>> {
    connection
        .prepare_cached("SELECT session_id, message_id, payload_hash FROM submission WHERE id = ?1")?
        .query_row([id], |row| {
            Ok(Submission {
                session_id: row.get(0)?,
                message_id: row.get(1)?,
                payload_hash: row.get(2)?,
            })
        })
        .optional()
}
