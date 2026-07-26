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
