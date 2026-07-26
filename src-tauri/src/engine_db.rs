use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SHARED_DATABASE_ENV: &str = "OPENCODE_DISABLE_CHANNEL_DB";
const LEGACY_DATABASE: &str = "opencode-master.db";
/// Independent legacy child rows only merge into a session the target actually keeps, so
/// children of a session deleted in the target are dropped instead of breaking its
/// foreign keys.
const SESSION_PARENT_EXISTS: &str = "EXISTS (
    SELECT 1 FROM main.session parent WHERE parent.id = source.session_id
)";

pub fn prepare_shared() -> Result<usize, String> {
    let source = database_path(false)?;
    let target = database_path(true)?;
    if source == target {
        return Ok(0);
    }
    if !target.is_file() {
        return Err(format!(
            "shared OpenCode database does not exist: {}",
            target.display()
        ));
    }
    if !source.is_file() {
        return Ok(0);
    }
    merge_sessions(&source, &target).map_err(|error| error.to_string())
}

pub fn database_path(shared: bool) -> Result<PathBuf, String> {
    Ok(opencode_data_dir()?.join(if shared {
        "opencode.db"
    } else {
        LEGACY_DATABASE
    }))
}

fn opencode_data_dir() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path).join("opencode"));
    }
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|home| home.join(".local").join("share").join("opencode"))
        .ok_or_else(|| "cannot resolve the OpenCode data directory".into())
}

fn merge_sessions(source: &Path, target: &Path) -> rusqlite::Result<usize> {
    let mut conn = Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute(
        "ATTACH DATABASE ?1 AS drift_channel",
        [source.to_string_lossy().as_ref()],
    )?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current_events =
        table_schemas_match(&tx, "event_sequence")? && table_schemas_match(&tx, "event")?;
    tx.execute_batch(
        "CREATE TEMP TABLE target_owned_session(
            session_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         CREATE TEMP TABLE source_session_aggregate(
            session_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;",
    )?;
    // The target owns a session id when it still holds either the session row or the
    // durable event aggregate. A session deleted in the target keeps its aggregate, so
    // the missing session row alone never makes the source copy importable.
    tx.execute_batch(
        "INSERT INTO temp.target_owned_session(session_id)
         SELECT source.id
         FROM drift_channel.session source
         WHERE EXISTS (
             SELECT 1 FROM main.session target WHERE target.id = source.id
         );",
    )?;
    if current_events {
        tx.execute_batch(
            "INSERT OR IGNORE INTO temp.target_owned_session(session_id)
             SELECT source.id
             FROM drift_channel.session source
             WHERE EXISTS (
                 SELECT 1 FROM main.event_sequence sequence
                 WHERE sequence.aggregate_id = source.id
             )
             OR EXISTS (
                 SELECT 1 FROM main.event event WHERE event.aggregate_id = source.id
             );",
        )?;
        tx.execute_batch(
            "INSERT INTO temp.source_session_aggregate(session_id)
             SELECT source.id
             FROM drift_channel.session source
             JOIN drift_channel.event_sequence sequence ON sequence.aggregate_id = source.id
             WHERE source.id NOT IN (SELECT session_id FROM temp.target_owned_session)
             AND sequence.seq >= 0
             AND (
                 SELECT COUNT(*) FROM drift_channel.event event
                 WHERE event.aggregate_id = source.id
             ) = sequence.seq + 1
             AND (
                 SELECT COUNT(DISTINCT event.seq) FROM drift_channel.event event
                 WHERE event.aggregate_id = source.id
                   AND event.seq BETWEEN 0 AND sequence.seq
             ) = sequence.seq + 1
             AND NOT EXISTS (
                 SELECT 1
                 FROM drift_channel.event event
                 JOIN main.event target ON target.id = event.id
                 WHERE event.aggregate_id = source.id
             );",
        )?;
    }

    let mut imported = 0;
    imported += copy_table(
        &tx,
        "project",
        true,
        Some("source.id IN (SELECT project_id FROM drift_channel.session)"),
    )?;
    imported += copy_table(
        &tx,
        "session",
        true,
        Some("source.id NOT IN (SELECT session_id FROM temp.target_owned_session)"),
    )?;
    if current_events {
        imported += copy_table(
            &tx,
            "event_sequence",
            false,
            Some(
                "source.aggregate_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                )",
            ),
        )?;
        imported += copy_table(
            &tx,
            "event",
            false,
            Some(
                "source.aggregate_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                )",
            ),
        )?;
    }
    imported += copy_table(&tx, "message", false, Some(SESSION_PARENT_EXISTS))?;
    imported += copy_table(
        &tx,
        "part",
        false,
        Some(
            "EXISTS (
                SELECT 1 FROM main.message parent
                WHERE parent.id = source.message_id AND parent.session_id = source.session_id
            )",
        ),
    )?;
    if current_events && table_schemas_match(&tx, "session_message")? {
        imported += copy_table(
            &tx,
            "session_message",
            false,
            Some(
                "source.session_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                 ) AND EXISTS (
                    SELECT 1 FROM drift_channel.event event
                    WHERE event.aggregate_id = source.session_id AND event.seq = source.seq
                 )",
            ),
        )?;
    }
    if current_events && table_schemas_match(&tx, "session_input")? {
        imported += copy_table(
            &tx,
            "session_input",
            false,
            Some(
                "source.session_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                 ) AND EXISTS (
                    SELECT 1 FROM drift_channel.event event
                    WHERE event.aggregate_id = source.session_id
                      AND event.seq = source.admitted_seq
                 ) AND (
                    source.promoted_seq IS NULL OR EXISTS (
                        SELECT 1 FROM drift_channel.event event
                        WHERE event.aggregate_id = source.session_id
                          AND event.seq = source.promoted_seq
                    )
                 )",
            ),
        )?;
    }
    if current_events && table_schemas_match(&tx, "session_context_epoch")? {
        imported += copy_table(
            &tx,
            "session_context_epoch",
            false,
            Some(
                "source.session_id IN (
                    SELECT session_id FROM temp.source_session_aggregate
                )",
            ),
        )?;
    }
    imported += copy_table(&tx, "todo", false, Some(SESSION_PARENT_EXISTS))?;
    imported += copy_table(&tx, "session_share", false, Some(SESSION_PARENT_EXISTS))?;
    verify_foreign_keys(&tx)?;
    tx.commit()?;
    Ok(imported)
}

struct Column {
    name: String,
    required: bool,
}

fn table_columns(tx: &Transaction<'_>, schema: &str, table: &str) -> rusqlite::Result<Vec<Column>> {
    let mut statement = tx.prepare(&format!(
        "PRAGMA {schema}.table_info({})",
        quote_identifier(table)
    ))?;
    let columns = statement
        .query_map([], |row| {
            let not_null = row.get::<_, bool>(3)?;
            let default = row.get::<_, Option<String>>(4)?;
            let primary_key = row.get::<_, i64>(5)? != 0;
            Ok(Column {
                name: row.get(1)?,
                required: primary_key || (not_null && default.is_none()),
            })
        })?
        .collect();
    columns
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn table_schemas_match(tx: &Transaction<'_>, table: &str) -> rusqlite::Result<bool> {
    let target = table_columns(tx, "main", table)?;
    let source = table_columns(tx, "drift_channel", table)?;
    Ok(!target.is_empty()
        && target.len() == source.len()
        && target
            .iter()
            .zip(source)
            .all(|(target, source)| target.name == source.name))
}

fn copy_table(
    tx: &Transaction<'_>,
    table: &str,
    required: bool,
    filter: Option<&str>,
) -> rusqlite::Result<usize> {
    let target_columns = table_columns(tx, "main", table)?;
    let source_columns = table_columns(tx, "drift_channel", table)?;
    if target_columns.is_empty() || source_columns.is_empty() {
        return if required {
            Err(rusqlite::Error::InvalidParameterName(format!(
                "required migration table is missing: {table}"
            )))
        } else {
            Ok(0)
        };
    }

    let source_names = source_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let mut insert_columns = Vec::new();
    let mut select_columns = Vec::new();
    for column in &target_columns {
        let source_name = source_names
            .contains(column.name.as_str())
            .then_some(column.name.as_str());
        if let Some(source_name) = source_name {
            insert_columns.push(quote_identifier(&column.name));
            select_columns.push(format!("source.{}", quote_identifier(source_name)));
        } else if column.required {
            return if required {
                Err(rusqlite::Error::InvalidParameterName(format!(
                    "source table {table} is missing required column {}",
                    column.name
                )))
            } else {
                Ok(0)
            };
        }
    }

    let table = quote_identifier(table);
    let filter = filter
        .map(|value| format!(" WHERE {value}"))
        .unwrap_or_default();
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO main.{table} ({}) SELECT {} FROM drift_channel.{table} source{filter}",
            insert_columns.join(", "),
            select_columns.join(", ")
        ),
        [],
    )
}

fn verify_foreign_keys(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let violation = {
        let mut statement = tx.prepare("PRAGMA foreign_key_check")?;
        let mut rows = statement.query([])?;
        if let Some(row) = rows.next()? {
            Some((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        } else {
            None
        }
    };
    if let Some((table, rowid, parent)) = violation {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
            Some(format!(
                "foreign key violation after legacy migration: {table} row {rowid:?} references {parent}"
            )),
        ));
    }
    Ok(())
}

pub fn configure_shared(command: &mut Command) {
    command.env(SHARED_DATABASE_ENV, "1");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DATABASE_ID: AtomicUsize = AtomicUsize::new(0);

    const CURRENT_SCHEMA: &str = "
        PRAGMA foreign_keys = ON;
        CREATE TABLE project(
            id TEXT PRIMARY KEY,
            worktree TEXT NOT NULL,
            vcs TEXT,
            name TEXT,
            icon_url TEXT,
            icon_url_override TEXT,
            icon_color TEXT,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            time_initialized INTEGER,
            sandboxes TEXT NOT NULL,
            commands TEXT
        );
        CREATE TABLE event_sequence(
            aggregate_id TEXT PRIMARY KEY,
            seq INTEGER NOT NULL,
            owner_id TEXT
        );
        CREATE TABLE event(
            id TEXT PRIMARY KEY,
            aggregate_id TEXT NOT NULL REFERENCES event_sequence(aggregate_id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            type TEXT NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE session(
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
            workspace_id TEXT,
            parent_id TEXT,
            slug TEXT NOT NULL,
            directory TEXT NOT NULL,
            path TEXT,
            title TEXT NOT NULL,
            version TEXT NOT NULL,
            share_url TEXT,
            summary_additions INTEGER,
            summary_deletions INTEGER,
            summary_files INTEGER,
            summary_diffs TEXT,
            metadata TEXT,
            cost REAL NOT NULL DEFAULT 0,
            tokens_input INTEGER NOT NULL DEFAULT 0,
            tokens_output INTEGER NOT NULL DEFAULT 0,
            tokens_reasoning INTEGER NOT NULL DEFAULT 0,
            tokens_cache_read INTEGER NOT NULL DEFAULT 0,
            tokens_cache_write INTEGER NOT NULL DEFAULT 0,
            revert TEXT,
            permission TEXT,
            agent TEXT,
            model TEXT,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            time_compacting INTEGER,
            time_archived INTEGER
        );
        CREATE TABLE message(
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE part(
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE session_message(
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
            type TEXT NOT NULL,
            seq INTEGER NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE session_input(
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
            prompt TEXT NOT NULL,
            delivery TEXT NOT NULL,
            admitted_seq INTEGER NOT NULL,
            promoted_seq INTEGER,
            time_created INTEGER NOT NULL
        );
        CREATE TABLE session_context_epoch(
            session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
            baseline TEXT NOT NULL,
            snapshot TEXT NOT NULL,
            baseline_seq INTEGER NOT NULL
        );
        CREATE TABLE todo(
            session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
            content TEXT NOT NULL,
            status TEXT NOT NULL,
            priority TEXT NOT NULL,
            position INTEGER NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            PRIMARY KEY(session_id, position)
        );
        CREATE TABLE session_share(
            session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
            id TEXT NOT NULL,
            secret TEXT NOT NULL,
            url TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX session_message_session_seq_idx
            ON session_message(session_id, seq);
        CREATE UNIQUE INDEX event_aggregate_seq_idx
            ON event(aggregate_id, seq);
        CREATE INDEX event_aggregate_type_seq_idx
            ON event(aggregate_id, type, seq);
        CREATE UNIQUE INDEX session_input_session_admitted_seq_idx
            ON session_input(session_id, admitted_seq);
        CREATE UNIQUE INDEX session_input_session_promoted_seq_idx
            ON session_input(session_id, promoted_seq);
    ";

    const LEGACY_SCHEMA_WITHOUT_OPTIONAL_TABLES: &str = "
        PRAGMA foreign_keys = ON;
        CREATE TABLE project(
            id TEXT PRIMARY KEY,
            worktree TEXT NOT NULL,
            vcs TEXT,
            name TEXT,
            icon_url TEXT,
            icon_color TEXT,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            time_initialized INTEGER,
            sandboxes TEXT NOT NULL,
            commands TEXT
        );
        CREATE TABLE session(
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES project(id),
            parent_id TEXT,
            slug TEXT NOT NULL,
            directory TEXT NOT NULL,
            title TEXT NOT NULL,
            version TEXT NOT NULL,
            share_url TEXT,
            summary_additions INTEGER,
            summary_deletions INTEGER,
            summary_files INTEGER,
            summary_diffs TEXT,
            revert TEXT,
            permission TEXT,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            time_compacting INTEGER,
            time_archived INTEGER
        );
        CREATE TABLE message(
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES session(id),
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE part(
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL REFERENCES message(id),
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE todo(
            session_id TEXT NOT NULL REFERENCES session(id),
            content TEXT NOT NULL,
            status TEXT NOT NULL,
            priority TEXT NOT NULL,
            position INTEGER NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            PRIMARY KEY(session_id, position)
        );
        CREATE TABLE session_input(
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            session_id TEXT NOT NULL REFERENCES session(id),
            prompt TEXT NOT NULL,
            delivery TEXT NOT NULL,
            promoted_seq INTEGER,
            time_created INTEGER NOT NULL
        );
    ";

    struct TestDatabases {
        dir: PathBuf,
        source: PathBuf,
        target: PathBuf,
    }

    impl Drop for TestDatabases {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn databases(source_schema: &str) -> TestDatabases {
        let id = NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("drift-engine-db-test-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.db");
        let target = dir.join("target.db");
        Connection::open(&source)
            .unwrap()
            .execute_batch(source_schema)
            .unwrap();
        Connection::open(&target)
            .unwrap()
            .execute_batch(CURRENT_SCHEMA)
            .unwrap();
        TestDatabases {
            dir,
            source,
            target,
        }
    }

    fn insert_project(conn: &Connection, worktree: &str) {
        conn.execute(
            "INSERT INTO project(id, worktree, time_created, time_updated, sandboxes)
             VALUES('project', ?1, 1, 1, '[]')",
            [worktree],
        )
        .unwrap();
    }

    fn insert_session(conn: &Connection, id: &str, title: &str) {
        conn.execute(
            "INSERT INTO session(id, project_id, slug, directory, title, version, time_created, time_updated)
             VALUES(?1, 'project', ?1, 'C:/work', ?2, '1', 1, 2)",
            [id, title],
        )
        .unwrap();
    }

    fn insert_aggregate(conn: &Connection, session_id: &str, value: &str) {
        conn.execute(
            "INSERT INTO event_sequence VALUES(?1, 2, ?2)",
            (session_id, format!("{value}-owner")),
        )
        .unwrap();
        for seq in 0..=2 {
            conn.execute(
                "INSERT INTO event(id, aggregate_id, seq, type, data)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                (
                    format!("{value}-event-{seq}"),
                    session_id,
                    seq,
                    format!("session.test-{seq}.1"),
                    value,
                ),
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO session_message VALUES(?1, ?2, 'user', 2, 1, 2, ?3)",
            (format!("{value}-session-message"), session_id, value),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_input VALUES(?1, ?2, ?3, 'steer', 1, 2, 1)",
            (format!("{value}-session-input"), session_id, value),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_context_epoch VALUES(?1, 'baseline', ?2, 2)",
            (session_id, value),
        )
        .unwrap();
    }

    fn assert_foreign_keys(conn: &Connection) {
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn imports_a_complete_current_aggregate_and_continues_its_sequence() {
        let databases = databases(CURRENT_SCHEMA);
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "imported", "Imported session");
        insert_aggregate(&source, "imported", "source");
        source
            .execute(
                "INSERT INTO session_input VALUES(
                    'orphan-input', 'imported', 'orphan', 'queue', 99, NULL, 1
                 )",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO message VALUES('message', 'imported', 1, 2, 'message')",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES('part', 'message', 'imported', 1, 2, 'part')",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO todo VALUES('imported', 'todo', 'pending', 'high', 0, 1, 2)",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session_share VALUES(
                    'imported', 'share', 'secret', 'https://example.com', 1, 2
                 )",
                [],
            )
            .unwrap();
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            13
        );
        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row(
                    "SELECT title FROM session WHERE id = 'imported'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Imported session"
        );
        assert_eq!(
            target
                .prepare("SELECT seq FROM event WHERE aggregate_id = 'imported' ORDER BY seq")
                .unwrap()
                .query_map([], |row| row.get::<_, i64>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap(),
            vec![0, 1, 2]
        );
        assert_eq!(
            target
                .query_row(
                    "SELECT COUNT(*) FROM session_input WHERE session_id = 'imported'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            target
                .query_row(
                    "UPDATE event_sequence SET seq = seq + 1
                     WHERE aggregate_id = 'imported' RETURNING seq",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            3
        );
        target
            .execute(
                "INSERT INTO event VALUES(
                    'next-event', 'imported', 3, 'session.test-next.1', 'next'
                 )",
                [],
            )
            .unwrap();
        assert_foreign_keys(&target);
    }

    #[test]
    fn accepts_an_older_schema_with_missing_optional_tables_and_columns() {
        let databases = databases(LEGACY_SCHEMA_WITHOUT_OPTIONAL_TABLES);
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "overlap", "Legacy session");
        source
            .execute(
                "INSERT INTO message VALUES('message', 'overlap', 1, 2, 'message')",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES('part', 'message', 'overlap', 1, 2, 'part')",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO todo VALUES('overlap', 'todo', 'pending', 'high', 0, 1, 2)",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session_input(seq, id, session_id, prompt, delivery, time_created)
                 VALUES(42, 'legacy-input', 'overlap', 'prompt', 'steer', 1)",
                [],
            )
            .unwrap();
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            5
        );
        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row("SELECT cost FROM session WHERE id = 'overlap'", [], |row| {
                    row.get::<_, f64>(0)
                },)
                .unwrap(),
            0.0
        );
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM session_input", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_foreign_keys(&target);
    }

    #[test]
    fn preserves_the_target_aggregate_but_merges_independent_legacy_rows() {
        let databases = databases(CURRENT_SCHEMA);
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "overlap", "Source session");
        insert_aggregate(&source, "overlap", "source");
        source
            .execute(
                "INSERT INTO message VALUES('source-message', 'overlap', 1, 2, 'source')",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES(
                    'source-part', 'source-message', 'overlap', 1, 2, 'source'
                 )",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO todo VALUES('overlap', 'source', 'pending', 'high', 1, 1, 2)",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session_share VALUES(
                    'overlap', 'source-share', 'source', 'https://source.example.com', 1, 2
                 )",
                [],
            )
            .unwrap();
        drop(source);

        let target = Connection::open(&databases.target).unwrap();
        insert_project(&target, "/target");
        insert_session(&target, "overlap", "Target session");
        insert_aggregate(&target, "overlap", "target");
        target
            .execute(
                "INSERT INTO message VALUES('target-message', 'overlap', 1, 2, 'target')",
                [],
            )
            .unwrap();
        target
            .execute(
                "INSERT INTO part VALUES(
                    'target-part', 'target-message', 'overlap', 1, 2, 'target'
                 )",
                [],
            )
            .unwrap();
        target
            .execute(
                "INSERT INTO todo VALUES('overlap', 'target', 'pending', 'high', 0, 1, 2)",
                [],
            )
            .unwrap();
        drop(target);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            4
        );
        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        for query in [
            "SELECT title FROM session WHERE id = 'overlap'",
            "SELECT data FROM event WHERE id = 'target-event-2'",
            "SELECT data FROM session_message WHERE id = 'target-session-message'",
            "SELECT prompt FROM session_input WHERE id = 'target-session-input'",
            "SELECT snapshot FROM session_context_epoch WHERE session_id = 'overlap'",
        ] {
            assert_eq!(
                target
                    .query_row(query, [], |row| row.get::<_, String>(0))
                    .unwrap(),
                if query.contains("title") {
                    "Target session"
                } else {
                    "target"
                }
            );
        }
        assert_eq!(
            target
                .query_row(
                    "SELECT COUNT(*) FROM event WHERE id LIKE 'source-event-%'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM message", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM part", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM todo", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert_eq!(
            target
                .query_row(
                    "SELECT secret FROM session_share WHERE session_id = 'overlap'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "source"
        );
        assert_foreign_keys(&target);
    }

    #[test]
    fn never_resurrects_a_deleted_session_that_still_owns_its_target_aggregate() {
        let databases = databases(CURRENT_SCHEMA);
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "deleted", "Source session");
        insert_aggregate(&source, "deleted", "source");
        // A complete source stream that runs past the sequence the target retained, so its
        // tail would graft onto the target aggregate instead of colliding with it.
        source
            .execute(
                "UPDATE event_sequence SET seq = 4 WHERE aggregate_id = 'deleted'",
                [],
            )
            .unwrap();
        for seq in 3..=4 {
            source
                .execute(
                    "INSERT INTO event(id, aggregate_id, seq, type, data)
                     VALUES(?1, 'deleted', ?2, 'session.test-tail.1', 'source')",
                    (format!("source-event-{seq}"), seq),
                )
                .unwrap();
        }
        source
            .execute(
                "INSERT INTO session_input VALUES(
                    'source-tail-input', 'deleted', 'source', 'steer', 3, 4, 1
                 )",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO message VALUES('source-message', 'deleted', 1, 2, 'source')",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES(
                    'source-part', 'source-message', 'deleted', 1, 2, 'source'
                 )",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO todo VALUES('deleted', 'source', 'pending', 'high', 0, 1, 2)",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session_share VALUES(
                    'deleted', 'source-share', 'source', 'https://source.example.com', 1, 2
                 )",
                [],
            )
            .unwrap();
        drop(source);

        let target = Connection::open(&databases.target).unwrap();
        target.pragma_update(None, "foreign_keys", "ON").unwrap();
        insert_project(&target, "/target");
        insert_session(&target, "deleted", "Target session");
        insert_aggregate(&target, "deleted", "target");
        // Deleting the session cascades its projections but keeps the durable aggregate.
        target
            .execute("DELETE FROM session WHERE id = 'deleted'", [])
            .unwrap();
        drop(target);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );
        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM session", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            target
                .query_row(
                    "SELECT seq || ':' || owner_id FROM event_sequence
                     WHERE aggregate_id = 'deleted'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "2:target-owner"
        );
        assert_eq!(
            target
                .prepare("SELECT id FROM event WHERE aggregate_id = 'deleted' ORDER BY seq")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap(),
            vec!["target-event-0", "target-event-1", "target-event-2"]
        );
        for table in [
            "event_sequence",
            "message",
            "part",
            "todo",
            "session_share",
            "session_message",
            "session_input",
            "session_context_epoch",
        ] {
            assert_eq!(
                target
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                if table == "event_sequence" { 1 } else { 0 },
                "unexpected {table} rows after migration"
            );
        }
        assert_foreign_keys(&target);
    }
}
