//! Worker records: one row per `task` call, keyed by the call that launched it.

use rusqlite::{params, OptionalExtension, Row};

use super::Store;
use crate::id;
use crate::session::tasks::{Mode, TaskRecord, TaskState};

const COLUMNS: &str = "id, parent_session_id, session_id, call_id, description, agent, mode, reason, state, result, delivered, created_at, finished_at";

/// What launching a worker records before it starts.
pub struct NewTask<'a> {
    pub parent_session_id: &'a str,
    pub session_id: &'a str,
    pub call_id: &'a str,
    pub description: &'a str,
    pub agent: &'a str,
    pub mode: Mode,
    pub reason: &'a str,
}

impl Store {
    /// Records a launch, or returns the one this call already made: a call launches one worker, ever.
    pub fn create_task(&self, new: NewTask) -> rusqlite::Result<(TaskRecord, bool)> {
        if let Some(existing) = self.task_for_call(new.parent_session_id, new.call_id)? {
            return Ok((existing, false));
        }
        let task_id = id::new("task");
        let state = if new.mode == Mode::Background { TaskState::Queued } else { TaskState::Running };
        self.lock()
            .prepare_cached(&format!("INSERT INTO task({COLUMNS}) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, 0, ?10, NULL)"))?
            .execute(params![task_id, new.parent_session_id, new.session_id, new.call_id, new.description, new.agent, new.mode.as_str(), new.reason, state.as_str(), id::now_ms()])?;
        Ok((self.task(&task_id)?.expect("just inserted"), true))
    }

    pub fn task(&self, task_id: &str) -> rusqlite::Result<Option<TaskRecord>> {
        self.lock().prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE id = ?1"))?.query_row([task_id], row).optional()
    }

    fn task_for_call(&self, parent: &str, call_id: &str) -> rusqlite::Result<Option<TaskRecord>> {
        self.lock()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE parent_session_id = ?1 AND call_id = ?2"))?
            .query_row([parent, call_id], row)
            .optional()
    }

    /// The worker whose transcript is `session_id`, if it is one.
    pub fn task_for_session(&self, session_id: &str) -> rusqlite::Result<Option<TaskRecord>> {
        self.lock().prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE session_id = ?1"))?.query_row([session_id], row).optional()
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

    pub fn mark_task_delivered(&self, task_id: &str) -> rusqlite::Result<()> {
        self.lock().prepare_cached("UPDATE task SET delivered = 1 WHERE id = ?1")?.execute([task_id])?;
        Ok(())
    }

    /// At startup, before anything can launch: whatever was still queued or running belongs to a
    /// process that is gone. It is marked interrupted and never rerun, and it wakes no one.
    pub fn interrupt_unfinished_tasks(&self) -> rusqlite::Result<usize> {
        self.lock()
            .prepare_cached(
                "UPDATE task SET state = 'interrupted', result = 'Drift stopped while the subagent ran; it was not restarted.', finished_at = ?1, delivered = 1
                 WHERE state IN ('queued', 'running')",
            )?
            .execute([id::now_ms()])
    }

    /// Finished workers whose result has not reached the parent.
    pub fn undelivered_tasks(&self) -> rusqlite::Result<Vec<TaskRecord>> {
        self.lock()
            .prepare_cached(&format!("SELECT {COLUMNS} FROM task WHERE delivered = 0 AND state NOT IN ('queued', 'running') ORDER BY id"))?
            .query_map([], row)?
            .collect()
    }
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::Visibility;
    use crate::store::{tests::store, NewSession};

    #[test]
    fn a_call_launches_one_worker_and_it_ends_once() {
        let store = store();
        let parent = store.create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
        let new = || NewTask { parent_session_id: &parent.id, session_id: "ses_child", call_id: "call_1", description: "d", agent: "general", mode: Mode::Background, reason: "explicit" };
        let (first, created) = store.create_task(new()).unwrap();
        assert!(created && first.state == TaskState::Queued);
        let (again, created) = store.create_task(new()).unwrap();
        assert!(!created && again.id == first.id, "the same call resolves to the same worker");
        assert!(store.start_task(&first.id).unwrap() && !store.start_task(&first.id).unwrap());
        assert!(store.finish_task(&first.id, TaskState::Replied, "done").unwrap());
        assert!(!store.finish_task(&first.id, TaskState::Stopped, "late").unwrap(), "a late ending does not overwrite the first");
        let done = store.task(&first.id).unwrap().unwrap();
        assert_eq!((done.state, done.result.as_deref(), done.delivered), (TaskState::Replied, Some("done"), false));
        assert_eq!(store.undelivered_tasks().unwrap().len(), 1);
        store.mark_task_delivered(&first.id).unwrap();
        assert!(store.undelivered_tasks().unwrap().is_empty());
        assert_eq!(store.task_for_session("ses_child").unwrap().unwrap().id, first.id);

        let second = NewTask { call_id: "call_2", session_id: "ses_other", ..new() };
        let (running, _) = store.create_task(second).unwrap();
        store.start_task(&running.id).unwrap();
        assert_eq!(store.interrupt_unfinished_tasks().unwrap(), 1);
        let gone = store.task(&running.id).unwrap().unwrap();
        assert_eq!((gone.state, gone.delivered), (TaskState::Interrupted, true), "never rerun, and it wakes no one");
        assert!(store.undelivered_tasks().unwrap().is_empty());
    }
}
