//! Background processes running now, written when one starts and removed when it ends, so one still
//! here at startup was ended by Drift stopping.

use rusqlite::params;

use super::Store;

/// A background process as kept while it runs: enough to tell its conversation it was cut off.
#[derive(Clone, Debug, PartialEq)]
pub struct RunningProcess {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub description: String,
    pub log: String,
    /// The conversation's Stop count when it started; a Stop since means nobody waits on it.
    pub generation: i64,
}

impl Store {
    pub fn note_process(&self, process: &RunningProcess) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached(
                "INSERT OR REPLACE INTO background_process(id, session_id, command, description, log, generation)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                process.id,
                process.session_id,
                process.command,
                process.description,
                process.log,
                process.generation
            ])?;

        Ok(())
    }

    pub fn forget_process(&self, id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("DELETE FROM background_process WHERE id = ?1")?
            .execute([id])?;

        Ok(())
    }

    /// The processes the last run left running, removed as they are taken.
    pub fn take_running_processes(&self) -> rusqlite::Result<Vec<RunningProcess>> {
        let conn = self.lock();
        let processes = conn
            .prepare_cached(
                "SELECT id, session_id, command, description, log, generation FROM background_process ORDER BY id",
            )?
            .query_map([], |row| {
                Ok(RunningProcess {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    command: row.get(2)?,
                    description: row.get(3)?,
                    log: row.get(4)?,
                    generation: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        conn.execute("DELETE FROM background_process", [])?;

        Ok(processes)
    }
}
