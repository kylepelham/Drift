use rusqlite::{Connection, OpenFlags, TransactionBehavior};
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

    let pending_sessions = conn.query_row(
        "SELECT COUNT(*) FROM drift_channel.session source
         WHERE NOT EXISTS (SELECT 1 FROM main.session target WHERE target.id = source.id)",
        [],
        |row| row.get::<_, usize>(0),
    )?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO main.project(
            id, worktree, vcs, name, icon_url, icon_color, time_created, time_updated,
            time_initialized, sandboxes, commands, icon_url_override
         )
         SELECT id, worktree, vcs, name, icon_url, icon_color, time_created, time_updated,
                time_initialized, sandboxes, commands, icon_url_override
         FROM drift_channel.project
         WHERE id IN (SELECT project_id FROM drift_channel.session);

         INSERT OR IGNORE INTO main.session(
            id, project_id, workspace_id, parent_id, slug, directory, path, title, version,
            share_url, summary_additions, summary_deletions, summary_files, summary_diffs,
            metadata, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read,
            tokens_cache_write, revert, permission, agent, model, time_created, time_updated,
            time_compacting, time_archived
         )
         SELECT id, project_id, workspace_id, parent_id, slug, directory, path, title, version,
                share_url, summary_additions, summary_deletions, summary_files, summary_diffs,
                metadata, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read,
                tokens_cache_write, revert, permission, agent, model, time_created, time_updated,
                time_compacting, time_archived
         FROM drift_channel.session;

         INSERT OR IGNORE INTO main.message(id, session_id, time_created, time_updated, data)
         SELECT id, session_id, time_created, time_updated, data FROM drift_channel.message;

         INSERT OR IGNORE INTO main.part(id, message_id, session_id, time_created, time_updated, data)
         SELECT id, message_id, session_id, time_created, time_updated, data FROM drift_channel.part;

         INSERT OR IGNORE INTO main.todo(
            session_id, content, status, priority, position, time_created, time_updated
         )
         SELECT session_id, content, status, priority, position, time_created, time_updated
         FROM drift_channel.todo;

         INSERT OR IGNORE INTO main.session_share(
            session_id, id, secret, url, time_created, time_updated
         )
         SELECT session_id, id, secret, url, time_created, time_updated
         FROM drift_channel.session_share;",
    )?;
    tx.commit()?;
    Ok(pending_sessions)
}

pub fn configure_shared(command: &mut Command) {
    command.env(SHARED_DATABASE_ENV, "1");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DATABASE_ID: AtomicUsize = AtomicUsize::new(0);

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

    fn schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE project(
                id TEXT PRIMARY KEY, worktree TEXT NOT NULL, vcs TEXT, name TEXT, icon_url TEXT,
                icon_color TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                time_initialized INTEGER, sandboxes TEXT NOT NULL, commands TEXT, icon_url_override TEXT
             );
             CREATE TABLE session(
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project(id), workspace_id TEXT,
                parent_id TEXT, slug TEXT NOT NULL, directory TEXT NOT NULL, path TEXT, title TEXT NOT NULL,
                version TEXT NOT NULL, share_url TEXT, summary_additions INTEGER, summary_deletions INTEGER,
                summary_files INTEGER, summary_diffs TEXT, metadata TEXT, cost REAL NOT NULL DEFAULT 0,
                tokens_input INTEGER NOT NULL DEFAULT 0, tokens_output INTEGER NOT NULL DEFAULT 0,
                tokens_reasoning INTEGER NOT NULL DEFAULT 0, tokens_cache_read INTEGER NOT NULL DEFAULT 0,
                tokens_cache_write INTEGER NOT NULL DEFAULT 0, revert TEXT, permission TEXT, agent TEXT,
                model TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                time_compacting INTEGER, time_archived INTEGER
             );
             CREATE TABLE message(
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES session(id),
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE TABLE part(
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES message(id), session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE TABLE todo(
                session_id TEXT NOT NULL REFERENCES session(id), content TEXT NOT NULL, status TEXT NOT NULL,
                priority TEXT NOT NULL, position INTEGER NOT NULL, time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL, PRIMARY KEY(session_id, position)
             );
             CREATE TABLE session_share(
                session_id TEXT PRIMARY KEY REFERENCES session(id), id TEXT NOT NULL, secret TEXT NOT NULL,
                url TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
             );",
        )
        .unwrap();
    }

    fn databases() -> TestDatabases {
        let id = NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("drift-engine-db-test-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.db");
        let target = dir.join("target.db");
        schema(&Connection::open(&source).unwrap());
        schema(&Connection::open(&target).unwrap());
        TestDatabases {
            dir,
            source,
            target,
        }
    }

    fn insert_project(conn: &Connection, worktree: &str) {
        conn.execute(
            "INSERT INTO project VALUES('p', ?1, NULL, NULL, NULL, NULL, 1, 1, NULL, '[]', NULL, NULL)",
            [worktree],
        )
        .unwrap();
    }

    fn insert_session(conn: &Connection, id: &str, title: &str) {
        conn.execute(
            "INSERT INTO session(id, project_id, slug, directory, title, version, time_created, time_updated)
             VALUES(?1, 'p', ?1, 'C:/work', ?2, '1', 1, 2)",
            [id, title],
        )
        .unwrap();
    }

    fn insert_message(conn: &Connection, id: &str, session_id: &str, data: &str) {
        conn.execute(
            "INSERT INTO message VALUES(?1, ?2, 1, 2, ?3)",
            [id, session_id, data],
        )
        .unwrap();
    }

    fn insert_part(conn: &Connection, id: &str, message_id: &str, session_id: &str, data: &str) {
        conn.execute(
            "INSERT INTO part VALUES(?1, ?2, ?3, 1, 2, ?4)",
            [id, message_id, session_id, data],
        )
        .unwrap();
    }

    fn overlapping_databases() -> TestDatabases {
        let databases = databases();
        let source = Connection::open(&databases.source).unwrap();
        insert_project(&source, "/source");
        insert_session(&source, "overlap", "Source session");
        let target = Connection::open(&databases.target).unwrap();
        insert_project(&target, "/target");
        insert_session(&target, "overlap", "Target session");
        databases
    }

    #[test]
    fn merges_messages_and_parts_when_all_sessions_overlap() {
        let databases = overlapping_databases();
        let source = Connection::open(&databases.source).unwrap();
        insert_message(&source, "message", "overlap", r#"{"role":"user"}"#);
        insert_part(
            &source,
            "part",
            "message",
            "overlap",
            r#"{"type":"text","text":"hello"}"#,
        );
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row("SELECT data FROM message WHERE id = 'message'", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            r#"{"role":"user"}"#
        );
        assert_eq!(
            target
                .query_row("SELECT data FROM part WHERE id = 'part'", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "{\"type\":\"text\",\"text\":\"hello\"}"
        );
    }

    #[test]
    fn merges_todos_and_shares_when_all_sessions_overlap() {
        let databases = overlapping_databases();
        let source = Connection::open(&databases.source).unwrap();
        source
            .execute(
                "INSERT INTO todo VALUES('overlap', 'keep this', 'pending', 'high', 0, 1, 2)",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session_share VALUES('overlap', 'share', 'secret', 'https://example.com/share', 1, 2)",
                [],
            )
            .unwrap();
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row(
                    "SELECT content FROM todo WHERE session_id = 'overlap' AND position = 0",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "keep this"
        );
        assert_eq!(
            target
                .query_row(
                    "SELECT secret FROM session_share WHERE session_id = 'overlap'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "secret"
        );
    }

    #[test]
    fn merges_children_for_overlapping_and_new_sessions() {
        let databases = overlapping_databases();
        let source = Connection::open(&databases.source).unwrap();
        insert_session(&source, "new", "New session");
        insert_message(&source, "overlap-message", "overlap", "overlap data");
        insert_message(&source, "new-message", "new", "new data");
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            1
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM session", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM message", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
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
    }

    #[test]
    fn repeated_merge_imports_children_added_later() {
        let databases = overlapping_databases();
        let source = Connection::open(&databases.source).unwrap();
        insert_message(&source, "first", "overlap", "first data");
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let source = Connection::open(&databases.source).unwrap();
        insert_message(&source, "second", "overlap", "second data");
        insert_part(&source, "second-part", "second", "overlap", "part data");
        drop(source);

        assert_eq!(
            merge_sessions(&databases.source, &databases.target).unwrap(),
            0
        );

        let target = Connection::open(&databases.target).unwrap();
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM message", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            target
                .query_row("SELECT COUNT(*) FROM part", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn merge_preserves_target_rows_on_collisions() {
        let databases = overlapping_databases();
        let source = Connection::open(&databases.source).unwrap();
        insert_message(&source, "message", "overlap", "source message");
        insert_part(&source, "part", "message", "overlap", "source part");
        source
            .execute(
                "INSERT INTO todo VALUES('overlap', 'source todo', 'pending', 'high', 0, 1, 2)",
                [],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session_share VALUES('overlap', 'source share', 'source secret', 'https://source', 1, 2)",
                [],
            )
            .unwrap();
        drop(source);

        let target = Connection::open(&databases.target).unwrap();
        insert_message(&target, "message", "overlap", "target message");
        insert_part(&target, "part", "message", "overlap", "target part");
        target
            .execute(
                "INSERT INTO todo VALUES('overlap', 'target todo', 'done', 'low', 0, 3, 4)",
                [],
            )
            .unwrap();
        target
            .execute(
                "INSERT INTO session_share VALUES('overlap', 'target share', 'target secret', 'https://target', 3, 4)",
                [],
            )
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
        for (query, expected) in [
            ("SELECT worktree FROM project WHERE id = 'p'", "/target"),
            (
                "SELECT title FROM session WHERE id = 'overlap'",
                "Target session",
            ),
            (
                "SELECT data FROM message WHERE id = 'message'",
                "target message",
            ),
            ("SELECT data FROM part WHERE id = 'part'", "target part"),
            (
                "SELECT content FROM todo WHERE session_id = 'overlap' AND position = 0",
                "target todo",
            ),
            (
                "SELECT secret FROM session_share WHERE session_id = 'overlap'",
                "target secret",
            ),
        ] {
            assert_eq!(
                target
                    .query_row(query, [], |row| row.get::<_, String>(0))
                    .unwrap(),
                expected
            );
        }
    }
}
