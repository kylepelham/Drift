use rusqlite::Connection;

/// Each entry runs once, in order, inside a transaction; `user_version` records how far we got.
const MIGRATIONS: [&str; 22] = [
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
    "ALTER TABLE session ADD COLUMN branch_cutoff TEXT;",
    "ALTER TABLE message ADD COLUMN summary INTEGER NOT NULL DEFAULT 0 CHECK(summary IN (0, 1));
    CREATE TABLE setting(
        key TEXT PRIMARY KEY,
        value_json TEXT NOT NULL
    ) STRICT;",
    "ALTER TABLE session ADD COLUMN revert_json TEXT;",
    "CREATE TABLE task(
        id TEXT PRIMARY KEY,
        parent_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
        session_id TEXT NOT NULL,
        call_id TEXT NOT NULL,
        description TEXT NOT NULL,
        agent TEXT NOT NULL,
        mode TEXT NOT NULL CHECK(mode IN ('foreground', 'background')),
        reason TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued', 'running', 'replied', 'failed', 'stopped', 'interrupted')),
        result TEXT,
        delivered INTEGER NOT NULL DEFAULT 0 CHECK(delivered IN (0, 1)),
        created_at INTEGER NOT NULL,
        finished_at INTEGER,
        UNIQUE(parent_session_id, call_id)
    ) STRICT;
    CREATE INDEX idx_task_parent ON task(parent_session_id, id);
    CREATE INDEX idx_task_pending ON task(state, delivered);",
    "ALTER TABLE task ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE stop_generation(
        session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
        generation INTEGER NOT NULL
    ) STRICT;",
    "CREATE TABLE staged_file(
        path TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL
    ) STRICT;",
    "ALTER TABLE task ADD COLUMN held INTEGER NOT NULL DEFAULT 0 CHECK(held IN (0, 1));",
    "ALTER TABLE task ADD COLUMN delivery_error TEXT;",
    "CREATE TABLE staged_replacement(
        staged TEXT PRIMARY KEY,
        destination TEXT NOT NULL,
        backup TEXT NOT NULL,
        created_at INTEGER NOT NULL
    ) STRICT;
    DROP TABLE staged_file;",
    "ALTER TABLE staged_replacement ADD COLUMN swapped INTEGER NOT NULL DEFAULT 0 CHECK(swapped IN (0, 1));",
    "ALTER TABLE session ADD COLUMN variant TEXT;",
    "ALTER TABLE message ADD COLUMN agent TEXT;
    UPDATE message SET agent = (SELECT agent FROM session WHERE session.id = message.session_id);",
    "CREATE TABLE queued_prompt(
        submission_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
        payload_hash TEXT NOT NULL,
        prompt_json TEXT NOT NULL,
        error TEXT,
        created_at INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX idx_queued_session ON queued_prompt(session_id);",
    "DROP TABLE queued_prompt;",
    "ALTER TABLE mcp_config DROP COLUMN approved_hash;
    DELETE FROM setting WHERE key = 'mcpApprovalKey';",
    "CREATE TABLE blob(
        hash TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        created_at INTEGER NOT NULL
    ) STRICT;",
    "CREATE TABLE blob_ref(
        hash TEXT NOT NULL,
        message_id TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
        PRIMARY KEY(hash, message_id)
    ) STRICT, WITHOUT ROWID;
    CREATE INDEX idx_blob_ref_message ON blob_ref(message_id);",
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
