//! Which conversations had a turn under way, written when the turn starts and removed when it ends,
//! so one still here at startup was cut off by Drift stopping, not by the user.

use rusqlite::params;

use super::Store;

/// A conversation whose turn was under way, and how often it has been resumed for its prompt.
pub struct TurnClaim {
    pub session_id: String,
    pub resumes: u32,
}

impl Store {
    /// Notes a turn starting; a claim already there (a resumed turn's) keeps its count.
    pub fn claim_turn(&self, session_id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("INSERT OR IGNORE INTO turn_claim(session_id, resumes) VALUES(?1, 0)")?
            .execute([session_id])?;

        Ok(())
    }

    pub fn release_turn(&self, session_id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("DELETE FROM turn_claim WHERE session_id = ?1")?
            .execute([session_id])?;

        Ok(())
    }

    pub fn claimed_turns(&self) -> rusqlite::Result<Vec<TurnClaim>> {
        self.lock()
            .prepare_cached("SELECT session_id, resumes FROM turn_claim ORDER BY session_id")?
            .query_map([], |row| {
                Ok(TurnClaim {
                    session_id: row.get(0)?,
                    resumes: row.get(1)?,
                })
            })?
            .collect()
    }

    pub fn count_resume(&self, session_id: &str) -> rusqlite::Result<()> {
        self.lock()
            .prepare_cached("UPDATE turn_claim SET resumes = resumes + 1 WHERE session_id = ?1")?
            .execute(params![session_id])?;

        Ok(())
    }
}
