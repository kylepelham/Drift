use super::Store;
use crate::id;
use crate::session::types::{ModelRef, Revert, Session, Visibility};
use rusqlite::{Connection, OptionalExtension, Row, params};

pub(super) use super::messages::{role_str, status_str};
pub(super) use super::parts::save_part_in;

#[cfg(test)]
#[path = "tests/sessions.rs"]
mod tests;

pub(super) const SESSION_COLUMNS: &str = "id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at, archived_at, branch_cutoff, revert_json, variant, auto_accept";

pub struct NewSession<'a> {
    pub workspace_id: &'a str,
    pub parent_id: Option<&'a str>,
    pub visibility: Visibility,
    pub title: &'a str,
    pub agent: &'a str,
    pub model: Option<&'a ModelRef>,
}

pub struct SessionFilter<'a> {
    pub workspace_id: Option<&'a str>,
    pub archived: bool,
    pub before: Option<&'a str>,
    pub limit: usize,
}

#[derive(Debug, PartialEq)]
pub enum Purge {
    Deleted,
    /// Restored since it was archived: kept.
    Active,
    Missing,
}

impl Store {
    pub fn create_session(&self, new: NewSession) -> rusqlite::Result<Session> {
        self.create_branch(new, None)
    }

    /// A session that records which source message it was cut from.
    pub fn create_branch(&self, new: NewSession, cutoff: Option<&str>) -> rusqlite::Result<Session> {
        let session = session_from(new, cutoff);
        insert_session(&self.lock(), &session)?;

        Ok(session)
    }

    pub fn session(&self, id: &str) -> rusqlite::Result<Option<Session>> {
        session_in(&self.lock(), id)
    }

    /// Listed newest first, subagents included; the UI nests them under their parent.
    pub fn sessions(&self, filter: SessionFilter) -> rusqlite::Result<Vec<Session>> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM session
             WHERE (?1 IS NULL OR workspace_id = ?1)
               AND (archived_at IS NOT NULL) = ?2
               AND (?3 IS NULL OR (updated_at, id) < (SELECT updated_at, id FROM session WHERE id = ?3))
             ORDER BY updated_at DESC, id DESC LIMIT ?4"
        ))?;
        let rows = statement.query_map(
            params![filter.workspace_id, filter.archived, filter.before, filter.limit as i64],
            map_session,
        )?;

        rows.collect()
    }

    pub fn update_session(
        &self,
        id: &str,
        title: Option<&str>,
        model: Option<&ModelRef>,
        agent: Option<&str>,
    ) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();
        connection
            .prepare_cached(
                "UPDATE session SET title = COALESCE(?2, title),
                model_provider = COALESCE(?3, model_provider), model_id = COALESCE(?4, model_id),
                agent = COALESCE(?6, agent),
                updated_at = ?5 WHERE id = ?1",
            )?
            .execute(params![
                id,
                title,
                model.map(|model| &model.provider),
                model.map(|model| &model.model),
                id::now_ms(),
                agent
            ])?;

        session_in(&connection, id)
    }

    /// Retitles only while the title is still `expected`, so a concurrent rename wins. `None` if it changed.
    pub fn retitle_if(&self, id: &str, expected: &str, title: &str) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();
        let changed = connection
            .prepare_cached("UPDATE session SET title = ?3, updated_at = ?4 WHERE id = ?1 AND title = ?2")?
            .execute(params![id, expected, title, id::now_ms()])?;
        if changed == 0 {
            return Ok(None);
        }

        session_in(&connection, id)
    }

    pub fn touch_session(&self, id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("UPDATE session SET updated_at = ?2 WHERE id = ?1")?
            .execute(params![id, id::now_ms()])?;

        Ok(())
    }

    pub fn set_session_archived(&self, id: &str, archived: bool) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();
        let timestamp = archived.then(id::now_ms);
        connection
            .prepare_cached("UPDATE session SET archived_at = ?2 WHERE id = ?1")?
            .execute(params![id, timestamp])?;

        session_in(&connection, id)
    }

    pub fn set_session_auto_accept(&self, id: &str, on: bool) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();
        connection
            .prepare_cached("UPDATE session SET auto_accept = ?2 WHERE id = ?1")?
            .execute(params![id, on])?;

        session_in(&connection, id)
    }

    /// Records a variant chosen outside a prompt, as when a retry is switched to another model.
    pub fn set_session_variant(&self, id: &str, variant: Option<&str>) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();
        connection
            .prepare_cached("UPDATE session SET variant = ?2, updated_at = ?3 WHERE id = ?1")?
            .execute(params![id, variant, id::now_ms()])?;

        session_in(&connection, id)
    }

    /// Sets or clears the session's undo marker.
    pub fn set_revert(&self, session_id: &str, revert: Option<&Revert>) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();
        connection
            .prepare_cached("UPDATE session SET revert_json = ?2, updated_at = ?3 WHERE id = ?1")?
            .execute(params![
                session_id,
                revert.map(|revert| serde_json::to_string(revert).unwrap()),
                id::now_ms()
            ])?;

        session_in(&connection, session_id)
    }

    /// Removes a session and everything under it, its subagents' sessions included. Archive first; this is the purge that follows.
    pub fn delete_session(&self, id: &str) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |connection| delete_tree(connection, id))
    }

    /// The archive purge: removes the session and its subagents only while it is still archived, in one write, so a restore cannot lose to it.
    pub fn purge_archived(&self, id: &str) -> rusqlite::Result<Purge> {
        transaction(&self.lock(), |connection| {
            let archived: Option<bool> = connection
                .prepare_cached("SELECT archived_at IS NOT NULL FROM session WHERE id = ?1")?
                .query_row([id], |row| row.get(0))
                .optional()?;

            match archived {
                Some(true) => delete_tree(connection, id).map(|_| Purge::Deleted),
                Some(false) => Ok(Purge::Active),
                None => Ok(Purge::Missing),
            }
        })
    }
}

pub(super) fn session_in(connection: &Connection, id: &str) -> rusqlite::Result<Option<Session>> {
    connection
        .prepare_cached(&format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1"))?
        .query_row([id], map_session)
        .optional()
}

pub(super) fn map_session(row: &Row) -> rusqlite::Result<Session> {
    let provider: Option<String> = row.get(6)?;
    let model: Option<String> = row.get(7)?;

    Ok(Session {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        parent_id: row.get(2)?,
        visibility: match row.get::<_, String>(3)?.as_str() {
            "hidden" => Visibility::Hidden,
            _ => Visibility::Sibling,
        },
        title: row.get(4)?,
        agent: row.get(5)?,
        model: provider
            .zip(model)
            .map(|(provider, model)| ModelRef { provider, model }),
        variant: row.get(13)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        archived_at: row.get(10)?,
        branch_cutoff: row.get(11)?,
        revert: row
            .get::<_, Option<String>>(12)?
            .and_then(|json| serde_json::from_str(&json).ok()),
        auto_accept: row.get(14)?,
        running: false,
    })
}

pub(super) fn visibility_str(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Hidden => "hidden",
        Visibility::Sibling => "sibling",
    }
}

pub(super) fn session_from(new: NewSession, cutoff: Option<&str>) -> Session {
    let now = id::now_ms();

    Session {
        id: id::new("ses"),
        workspace_id: new.workspace_id.into(),
        parent_id: new.parent_id.map(Into::into),
        visibility: new.visibility,
        title: new.title.into(),
        agent: new.agent.into(),
        model: new.model.cloned(),
        variant: None,
        created_at: now,
        updated_at: now,
        archived_at: None,
        branch_cutoff: cutoff.map(Into::into),
        revert: None,
        auto_accept: false,
        running: false,
    }
}

pub(super) fn insert_session(connection: &Connection, session: &Session) -> rusqlite::Result<()> {
    connection.prepare_cached(
        "INSERT INTO session(id, workspace_id, parent_id, visibility, title, agent, model_provider, model_id, created_at, updated_at, branch_cutoff)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
    )?.execute(params![session.id, session.workspace_id, session.parent_id, visibility_str(session.visibility), session.title,
        session.agent, session.model.as_ref().map(|model| &model.provider), session.model.as_ref().map(|model| &model.model),
        session.created_at, session.updated_at, session.branch_cutoff])?;

    Ok(())
}

/// Runs `f` inside one transaction: all of its writes land, or none do.
pub(super) fn transaction<T>(
    connection: &Connection,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    connection.execute_batch("BEGIN")?;

    match f(connection) {
        Ok(value) => {
            connection.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Deletes `id` and its hidden subagent sessions at any depth; spawned threads are independent and stay.
fn delete_tree(connection: &Connection, id: &str) -> rusqlite::Result<bool> {
    let deleted = connection
        .prepare_cached(
            "DELETE FROM session WHERE id IN (
             WITH RECURSIVE tree(id) AS (
                 SELECT id FROM session WHERE id = ?1
                 UNION SELECT s.id FROM session s JOIN tree ON s.parent_id = tree.id WHERE s.visibility = 'hidden'
             ) SELECT id FROM tree
         )",
        )?
        .execute([id])?;

    Ok(deleted > 0)
}
