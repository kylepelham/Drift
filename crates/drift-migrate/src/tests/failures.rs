use super::*;

#[test]
fn a_failed_parent_discards_its_partial_import_and_its_child_waits_for_a_retry() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_parent", None, "C:/repo", None);
    session(&conn, "ses_child", Some("ses_parent"), "D:/elsewhere", None);
    conn.execute_batch("DROP TABLE todo").unwrap();
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);
    let mut progress = Vec::new();

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |step| {
        let description = match step {
            Progress::Planned(count) => format!("planned {count}"),
            Progress::Finished(session) => format!("finished {}", session.is_some()),
        };
        progress.push(description);
    })
    .unwrap();

    assert_eq!(
        report,
        Report {
            failed: vec![
                ("ses_parent".into(), "no such table: todo".into()),
                (
                    "ses_child".into(),
                    "the conversation that started it did not import".into()
                ),
            ],
            ..Default::default()
        }
    );
    assert_eq!(progress, ["planned 2", "finished false", "finished false"]);
    for id in ["ses_parent", "ses_child"] {
        assert!(store.session(id).unwrap().is_none());
        assert!(!store.was_imported(id).unwrap());
    }

    let conn = Connection::open(&source).unwrap();
    conn.execute_batch(
        "CREATE TABLE todo(
             session_id TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, priority TEXT NOT NULL,
             position INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
         );",
    )
    .unwrap();
    drop(conn);

    let retry = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!(retry.imported, 2);
    assert!(retry.failed.is_empty());
    assert_eq!(
        store.session("ses_child").unwrap().unwrap().workspace_id,
        store.session("ses_parent").unwrap().unwrap().workspace_id
    );
}
