//! Worker records, one per launching call, and each owner's durable count of Stops.

use rusqlite::{params, Connection, OptionalExtension, Row};

use super::sessions::{insert_session, session_from, session_in, transaction};
use super::{NewSession, Store};
use crate::id;
use crate::session::tasks::{Mode, TaskRecord, TaskState};
use crate::session::types::{PartRow, Session};

const COLUMNS: &str = "id, parent_session_id, session_id, call_id, description, agent, mode, reason, state, result, delivered, created_at, finished_at, generation, held, delivery_error";

/// What launching a worker records before it starts.
pub struct NewTask<'a> {
    pub parent_session_id: &'a str,
    pub call_id: &'a str,
    pub description: &'a str,
    pub agent: &'a str,
    pub mode: Mode,
    pub reason: &'a str,
    /// The owner's Stop count at launch; a Stop since then keeps the result from waking it.
    pub generation: i64,
}

/// A launch as recorded: the worker, its transcript, and whether this call made them.
pub struct Launch {
    pub task: TaskRecord,
    pub child: Session,
    pub created: bool,
}

impl Store {
    /// Records a launch and its child session in one write, or returns the pair this call already made.
    pub fn launch_task(&self, new: NewTask, child: NewSession) -> rusqlite::Result<Launch> {
        transaction(&self.lock(), |conn| {
            if let Some(task) = query_one(conn, "parent_session_id = ?1 AND call_id = ?2", &[new.parent_session_id, new.call_id])? {
                let child = session_in(conn, &task.session_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                return Ok(Launch { task, child, created: false });
            }
            let session = session_from(child, None);
            insert_session(conn, &session)?;
            let task_id = id::new("task");
            let state = if new.mode == Mode::Background { TaskState::Queued } else { TaskState::Running };
            conn.prepare_cached(&format!("INSERT INTO task({COLUMNS}) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, 0, ?10, NULL, ?11, 0, NULL)"))?
                .execute(params![task_id, new.parent_session_id, session.id, new.call_id, new.description, new.agent, new.mode.as_str(), new.reason, state.as_str(), id::now_ms(), new.generation])?;
            let task = query_one(conn, "id = ?1", &[&task_id])?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(Launch { task, child: session, created: true })
        })
    }

    pub fn task(&self, task_id: &str) -> rusqlite::Result<Option<TaskRecord>> {
        query_one(&self.lock(), "id = ?1", &[task_id])
    }

    /// The worker whose transcript is `session_id`, if it is one.
    pub fn task_for_session(&self, session_id: &str) -> rusqlite::Result<Option<TaskRecord>> {
        query_one(&self.lock(), "session_id = ?1", &[session_id])
    }

    /// Every worker a session launched, oldest first.
    pub fn tasks_of(&self, parent: &str) -> rusqlite::Result<Vec<TaskRecord>> {
        self.lock().prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE parent_session_id = ?1 ORDER BY id"))?.query_map([parent], row)?.collect()
    }

    /// Moves a queued worker to running; nothing else moves backwards or sideways.
    pub fn start_task(&self, task_id: &str) -> rusqlite::Result<bool> {
        let changed = self.lock().prepare_cached("UPDATE task SET state = 'running' WHERE id = ?1 AND state = 'queued'")?.execute([task_id])?;
        Ok(changed == 1)
    }

    /// Records how a worker ended, once: `false` if it had already ended.
    pub fn finish_task(&self, task_id: &str, state: TaskState, result: &str) -> rusqlite::Result<bool> {
        let changed = self
            .lock()
            .prepare_cached("UPDATE task SET state = ?2, result = ?3, finished_at = ?4 WHERE id = ?1 AND state IN ('queued', 'running')")?
            .execute(params![task_id, state.as_str(), result, id::now_ms()])?;
        Ok(changed == 1)
    }

    /// Records that a call's own result already holds this one, for a row from before that was transactional.
    pub fn mark_task_delivered(&self, task_id: &str) -> rusqlite::Result<()> {
        self.lock().prepare_cached("UPDATE task SET delivered = 1 WHERE id = ?1")?.execute([task_id])?;
        Ok(())
    }

    /// Keeps a finished result from waking its parent; it stays owed for the parent's next prompt.
    pub fn hold_task(&self, task_id: &str) -> rusqlite::Result<bool> {
        let changed = self.lock().prepare_cached("UPDATE task SET held = 1, delivery_error = NULL WHERE id = ?1 AND delivered = 0 AND held = 0")?.execute([task_id])?;
        Ok(changed == 1)
    }

    /// Why an owed result could not be handed over yet; it stays owed.
    pub fn set_delivery_error(&self, task_id: &str, reason: &str) -> rusqlite::Result<()> {
        self.lock().prepare_cached("UPDATE task SET delivery_error = ?2 WHERE id = ?1 AND delivered = 0")?.execute(params![task_id, reason])?;
        Ok(())
    }

    /// Finished background results still owed to `parent` (or to anyone) that may wake it.
    pub fn owed_background(&self, parent: Option<&str>) -> rusqlite::Result<Vec<TaskRecord>> {
        self.lock()
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM task WHERE (?1 IS NULL OR parent_session_id = ?1) AND mode = 'background' AND delivered = 0 AND held = 0 AND state NOT IN ('queued', 'running') ORDER BY id"
            ))?
            .query_map([parent], row)?
            .collect()
    }

    /// Finished background results held back from `parent`, oldest first.
    pub fn held_tasks(&self, parent: &str) -> rusqlite::Result<Vec<TaskRecord>> {
        self.lock()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE parent_session_id = ?1 AND held = 1 AND delivered = 0 AND mode = 'background' ORDER BY id"))?
            .query_map([parent], row)?
            .collect()
    }

    /// Saves a call's result and marks `task_id` handed over in the same write; `false` if it already was.
    pub fn save_part_delivering(&self, row: &PartRow, task_id: &str) -> rusqlite::Result<bool> {
        transaction(&self.lock(), |conn| {
            super::sessions::save_part_in(conn, row)?;
            acknowledge(conn, task_id, &row.session_id)
        })
    }

    /// At startup: workers of a process that is gone are interrupted, never rerun, and wake no one.
    pub fn interrupt_unfinished_tasks(&self) -> rusqlite::Result<usize> {
        self.lock()
            .prepare_cached(
                "UPDATE task SET state = 'interrupted', result = 'Drift stopped while the subagent ran; it was not restarted.', finished_at = ?1, delivered = 1
                 WHERE state IN ('queued', 'running')",
            )?
            .execute([id::now_ms()])
    }

    /// Finished workers whose result has not reached the parent and is not held back from waking it.
    pub fn undelivered_tasks(&self) -> rusqlite::Result<Vec<TaskRecord>> {
        self.lock()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE delivered = 0 AND held = 0 AND state NOT IN ('queued', 'running') ORDER BY id"))?
            .query_map([], row)?
            .collect()
    }

    /// How many times the session has been stopped, ever.
    pub fn stop_generation(&self, session_id: &str) -> rusqlite::Result<i64> {
        let found = self.lock().prepare_cached("SELECT generation FROM stop_generation WHERE session_id = ?1")?.query_row([session_id], |r| r.get(0)).optional()?;
        Ok(found.unwrap_or(0))
    }

    /// Counts one more Stop, durably, and returns the new count.
    pub fn bump_stop_generation(&self, session_id: &str) -> rusqlite::Result<i64> {
        self.lock()
            .prepare_cached("INSERT INTO stop_generation(session_id, generation) VALUES(?1, 1) ON CONFLICT(session_id) DO UPDATE SET generation = generation + 1 RETURNING generation")?
            .query_row([session_id], |r| r.get(0))
    }
}

/// Marks a finished task of `parent` handed over. Called inside the write that saves what carried it.
pub(super) fn acknowledge(conn: &Connection, task_id: &str, parent: &str) -> rusqlite::Result<bool> {
    let changed = conn
        .prepare_cached("UPDATE task SET delivered = 1, delivery_error = NULL WHERE id = ?1 AND parent_session_id = ?2 AND delivered = 0 AND state NOT IN ('queued', 'running')")?
        .execute([task_id, parent])?;
    Ok(changed == 1)
}

fn query_one(conn: &Connection, filter: &str, args: &[&str]) -> rusqlite::Result<Option<TaskRecord>> {
    conn.prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE {filter}"))?.query_row(rusqlite::params_from_iter(args), row).optional()
}

fn row(row: &Row) -> rusqlite::Result<TaskRecord> {
    Ok(TaskRecord {
        id: row.get(0)?,
        parent_session_id: row.get(1)?,
        session_id: row.get(2)?,
        call_id: row.get(3)?,
        description: row.get(4)?,
        agent: row.get(5)?,
        mode: Mode::parse(&row.get::<_, String>(6)?),
        reason: row.get(7)?,
        state: TaskState::parse(&row.get::<_, String>(8)?),
        result: row.get(9)?,
        delivered: row.get::<_, i64>(10)? == 1,
        created_at: row.get(11)?,
        finished_at: row.get(12)?,
        generation: row.get(13)?,
        held: row.get::<_, i64>(14)? == 1,
        delivery_error: row.get(15)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::session::types::Visibility;
    use crate::store::tests::store;

    pub(crate) fn child<'a>(parent: &'a Session) -> NewSession<'a> {
        NewSession { workspace_id: &parent.workspace_id, parent_id: Some(&parent.id), visibility: Visibility::Hidden, title: "worker", agent: "general", model: None }
    }

    pub(crate) fn new_task<'a>(parent: &'a str, call_id: &'a str, mode: Mode) -> NewTask<'a> {
        NewTask { parent_session_id: parent, call_id, description: call_id, agent: "general", mode, reason: "requested", generation: 0 }
    }

    #[test]
    fn a_call_launches_one_worker_and_one_transcript_and_it_ends_once() {
        let store = store();
        let parent = store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
        let first = store.launch_task(new_task(&parent.id, "call_1", Mode::Background), child(&parent)).unwrap();
        assert!(first.created && first.task.state == TaskState::Queued && first.task.session_id == first.child.id);
        let again = store.launch_task(new_task(&parent.id, "call_1", Mode::Foreground), child(&parent)).unwrap();
        assert!(!again.created && again.task.id == first.task.id && again.child.id == first.child.id, "the same call resolves to the same worker and transcript");
        assert_eq!(again.task.mode, Mode::Background, "as it was launched, not as the replay asked");
        let children = store.sessions(crate::store::SessionFilter { workspace_id: Some("w"), archived: false, before: None, limit: 10 }).unwrap();
        assert_eq!(children.iter().filter(|s| s.parent_id.as_deref() == Some(parent.id.as_str())).count(), 1, "no orphan transcript");

        let id = &first.task.id;
        assert!(store.start_task(id).unwrap() && !store.start_task(id).unwrap());
        assert!(store.finish_task(id, TaskState::Replied, "done").unwrap());
        assert!(!store.finish_task(id, TaskState::Stopped, "late").unwrap(), "a late ending does not overwrite the first");
        let done = store.task(id).unwrap().unwrap();
        assert_eq!((done.state, done.result.as_deref(), done.delivered), (TaskState::Replied, Some("done"), false));
        assert_eq!(store.undelivered_tasks().unwrap().len(), 1);
        store.mark_task_delivered(id).unwrap();
        assert!(store.undelivered_tasks().unwrap().is_empty());
        assert_eq!(store.task_for_session(&first.child.id).unwrap().unwrap().id, *id);

        let second = store.launch_task(new_task(&parent.id, "call_2", Mode::Background), child(&parent)).unwrap();
        store.start_task(&second.task.id).unwrap();
        assert_eq!(store.interrupt_unfinished_tasks().unwrap(), 1);
        let gone = store.task(&second.task.id).unwrap().unwrap();
        assert_eq!((gone.state, gone.delivered), (TaskState::Interrupted, true), "never rerun, and it wakes no one");
        assert!(store.undelivered_tasks().unwrap().is_empty());
    }

    #[test]
    fn stops_are_counted_durably_per_session() {
        let store = store();
        assert_eq!(store.stop_generation("ses_a").unwrap(), 0);
        let parent = store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
        assert_eq!(store.bump_stop_generation(&parent.id).unwrap(), 1);
        assert_eq!(store.bump_stop_generation(&parent.id).unwrap(), 2);
        assert_eq!(store.stop_generation(&parent.id).unwrap(), 2);
    }
}
