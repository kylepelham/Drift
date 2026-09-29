use rusqlite::Connection;

/// Each entry runs once, in order, inside a transaction; `user_version` records how far we got.
const MIGRATIONS: [&str; 5] = [
    "CREATE TABLE IF NOT EXISTS workspace(
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        icon TEXT NOT NULL DEFAULT '',
        last_used INTEGER NOT NULL DEFAULT 0,
        removed_at INTEGER
    ) STRICT;",
    "CREATE TABLE session(
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        parent_id TEXT,
        visibility TEXT NOT NULL CHECK(visibility IN ('hidden', 'sibling')),
        title TEXT NOT NULL,
        agent TEXT NOT NULL,
        model_provider TEXT,
        model_id TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        archived_at INTEGER
    ) STRICT;
    CREATE INDEX idx_session_workspace ON session(workspace_id, archived_at, updated_at);
    CREATE INDEX idx_session_parent ON session(parent_id);
    CREATE TABLE message(
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
        role TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
        status TEXT NOT NULL,
        model_provider TEXT,
        model_id TEXT,
        usage_json TEXT NOT NULL,
        cost REAL NOT NULL DEFAULT 0,
        error TEXT,
        created_at INTEGER NOT NULL,
        finished_at INTEGER
    ) STRICT;
    CREATE INDEX idx_message_session ON message(session_id, id);
    CREATE TABLE part(
        id TEXT PRIMARY KEY,
        message_id TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
        session_id TEXT NOT NULL,
        json TEXT NOT NULL
    ) STRICT;
    CREATE INDEX idx_part_message ON part(message_id, id);",
    "CREATE TABLE todo(
        session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
        json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    ) STRICT;",
    "CREATE TABLE submission(
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
        message_id TEXT NOT NULL,
        payload_hash TEXT NOT NULL,
        created_at INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX idx_session_order ON session(workspace_id, archived_at, updated_at DESC, id DESC);",
    "CREATE TABLE mcp_config(
        name TEXT PRIMARY KEY,
        config_json TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
        approved_hash TEXT,
        updated_at INTEGER NOT NULL
    ) STRICT;",
];

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
