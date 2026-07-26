use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SHARED_DATABASE_ENV: &str = "OPENCODE_DISABLE_CHANNEL_DB";
const LEGACY_DATABASE: &str = "opencode-master.db";

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
    let mut imported = 0;
    imported += copy_table(
        &tx,
        "project",
        true,
        &[],
        Some("source.id IN (SELECT project_id FROM drift_channel.session)"),
    )?;
    imported += copy_table(&tx, "session", true, &[], None)?;
    imported += copy_table(&tx, "message", false, &[], None)?;
    imported += copy_table(
        &tx,
        "part",
        false,
        &[],
        Some(
            "EXISTS (
                SELECT 1 FROM main.message parent
                WHERE parent.id = source.message_id AND parent.session_id = source.session_id
            )",
        ),
    )?;
    imported += copy_table(&tx, "session_message", false, &[], None)?;
    imported += copy_table(
        &tx,
        "session_input",
        false,
        &[("admitted_seq", "seq")],
        None,
    )?;
    imported += copy_table(&tx, "session_context_epoch", false, &[], None)?;
    imported += copy_table(&tx, "todo", false, &[], None)?;
    imported += copy_table(&tx, "session_share", false, &[], None)?;
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

fn copy_table(
    tx: &Transaction<'_>,
    table: &str,
    required: bool,
    aliases: &[(&str, &str)],
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
        let source_name = if source_names.contains(column.name.as_str()) {
            Some(column.name.as_str())
        } else {
            aliases
                .iter()
                .find(|(target, source)| *target == column.name && source_names.contains(*source))
                .map(|(_, source)| *source)
        };
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

    fn insert_descendants(conn: &Connection, value: &str) {
        conn.execute(
            "INSERT INTO message VALUES('message', 'overlap', 1, 2, ?1)",
            [value],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES('part', 'message', 'overlap', 1, 2, ?1)",
            [value],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message VALUES('session-message', 'overlap', 'user', 7, 1, 2, ?1)",
            [value],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_input VALUES('session-input', 'overlap', ?1, 'steer', 8, NULL, 1)",
            [value],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_context_epoch VALUES('overlap', 'baseline', ?1, 7)",
            [value],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO todo VALUES('overlap', ?1, 'pending', 'high', 0, 1, 2)",
            [value],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_share VALUES('overlap', 'share', ?1, 'https://example.com', 1, 2)",
            [value],
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
    fn imports_all_descendants_for_an_overlapping_session_and_reports_rows() {
        let databases = databases(CURRENT_SCHEMA);
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "overlap", "Source session");
        insert_descendants(&source, "source");
        drop(source);

        let target = Connection::open(&databases.target).unwrap();
        insert_project(&target, "/target");
        insert_session(&target, "overlap", "Target session");
        drop(target);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            7
        );

        let target = Connection::open(&databases.target).unwrap();
        for table in [
            "message",
            "part",
            "session_message",
            "session_input",
            "session_context_epoch",
            "todo",
            "session_share",
        ] {
            assert_eq!(
                target
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                1,
                "missing row in {table}"
            );
        }
        assert_eq!(
            target
                .query_row(
                    "SELECT title FROM session WHERE id = 'overlap'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Target session"
        );
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
            6
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
                .query_row(
                    "SELECT admitted_seq FROM session_input WHERE id = 'legacy-input'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            42
        );
        assert_foreign_keys(&target);
    }

    #[test]
    fn preserves_all_target_descendants_on_collisions_and_is_idempotent() {
        let databases = databases(CURRENT_SCHEMA);
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "overlap", "Source session");
        insert_descendants(&source, "source");
        drop(source);

        let target = Connection::open(&databases.target).unwrap();
        insert_project(&target, "/target");
        insert_session(&target, "overlap", "Target session");
        insert_descendants(&target, "target");
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
        for query in [
            "SELECT data FROM message WHERE id = 'message'",
            "SELECT data FROM part WHERE id = 'part'",
            "SELECT data FROM session_message WHERE id = 'session-message'",
            "SELECT prompt FROM session_input WHERE id = 'session-input'",
            "SELECT snapshot FROM session_context_epoch WHERE session_id = 'overlap'",
            "SELECT content FROM todo WHERE session_id = 'overlap' AND position = 0",
            "SELECT secret FROM session_share WHERE session_id = 'overlap'",
        ] {
            assert_eq!(
                target
                    .query_row(query, [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "target"
            );
        }
        assert_foreign_keys(&target);
    }
}
