use rusqlite::Connection;

/// Each entry runs once, in order, inside a transaction; `user_version` records how far we got.
const MIGRATIONS: [&str; 1] = ["CREATE TABLE IF NOT EXISTS workspace(
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        icon TEXT NOT NULL DEFAULT '',
        last_used INTEGER NOT NULL DEFAULT 0,
        removed_at INTEGER
    ) STRICT;"];

#[cfg(test)]
pub const LATEST: i64 = MIGRATIONS.len() as i64;

pub fn apply(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = index as i64 + 1;
        conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {version}; COMMIT;"))?;
    }
    Ok(())
}
