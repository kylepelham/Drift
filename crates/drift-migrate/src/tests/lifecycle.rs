use super::*;

fn assert_subagent_workspace(store: &Store) {
    let child = store.session("ses_child").unwrap().unwrap();

    assert_eq!(
        (child.visibility, child.parent_id.as_deref()),
        (Visibility::Hidden, Some("ses_parent"))
    );
    assert_eq!(
        child.workspace_id,
        store.session("ses_parent").unwrap().unwrap().workspace_id,
        "a subagent lands with its parent"
    );
}

#[test]
fn subagents_follow_their_parent_and_a_rerun_brings_in_only_what_is_new() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_child", Some("ses_parent"), "D:/elsewhere", None);
    session(&conn, "ses_parent", None, "C:/repo", None);
    session(&conn, "ses_away", None, "E:/other", None);
    session(&conn, "ses_old", None, "C:/repo", Some(500));
    session(&conn, "ses_hidden", None, "C:/repo", None);
    session(&conn, "ses_sub", None, "C:/repo/crates/core", None);
    conn.execute_batch(
        "INSERT INTO project VALUES('p_repo', 'C:/repo');
         UPDATE session SET project_id = 'p_repo', time_updated = 9999 WHERE id = 'ses_sub';",
    )
    .unwrap();
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    let before = drift_engine::id::now_ms();
    let mut announced = Vec::new();
    let first = run_import(
        &store,
        &source,
        &HashSet::from(["ses_hidden".to_string()]),
        &mut Kept::default(),
        &mut |step| {
            if let Progress::Finished(Some(session)) = step {
                announced.push(session.id.clone());
            }
        },
    )
    .unwrap();

    assert_eq!(
        (first.imported, first.unmatched.get("E:/other")),
        (5, Some(&1)),
        "{first:?}"
    );
    assert_eq!(
        (announced.len(), announced.last().map(String::as_str)),
        (5, Some("ses_child")),
        "each announced as it lands, a subagent after its parent"
    );
    assert_eq!(announced[0], "ses_sub", "the most recently used first");
    assert_eq!(
        store.session("ses_sub").unwrap().unwrap().workspace_id,
        store.workspaces().unwrap()[0].id,
        "run inside the repository the workspace holds"
    );
    assert_subagent_workspace(&store);
    for archived in ["ses_old", "ses_hidden"] {
        assert!(
            store.session(archived).unwrap().unwrap().archived_at.unwrap() >= before,
            "{archived}: the week counts from the import"
        );
    }

    store
        .lock()
        .execute("DELETE FROM session WHERE id = 'ses_parent'", [])
        .unwrap();
    store.add_workspace("e:\\other", "other", "").unwrap();
    let second = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!(
        (second.imported, second.known, second.unmatched.len()),
        (1, 5, 0),
        "{second:?}"
    );
    assert!(
        store.session("ses_away").unwrap().is_some(),
        "a workspace added since brings its conversations in"
    );
    assert!(
        store.session("ses_parent").unwrap().is_none(),
        "a deleted import stays deleted"
    );
}

#[test]
fn a_conversation_a_stopped_run_left_half_written_is_finished_by_the_next_and_progress_counts_every_one() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    session(&conn, "ses_b", None, "C:/repo", None);
    Conversation::new(&conn, "ses_a").message("msg_1", 2000, user(2000), &[json!({ "type": "text", "text": "hello" })]);
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);
    let half = drift_engine::session::types::Session {
        id: "ses_a".into(),
        workspace_id: store.workspaces().unwrap()[0].id.clone(),
        parent_id: None,
        visibility: Visibility::Sibling,
        title: "half".into(),
        agent: "build".into(),
        model: None,
        variant: None,
        created_at: 1,
        updated_at: 1,
        archived_at: None,
        branch_cutoff: None,
        revert: None,
        auto_accept: false,
        running: false,
    };
    assert!(
        store.begin_import(&half).unwrap(),
        "a run that stopped after starting this one"
    );

    let mut steps = Vec::new();
    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |step| {
        steps.push(match step {
            Progress::Planned(total) => format!("planned {total}"),
            Progress::Finished(session) => format!("finished {}", session.is_some()),
        });
    })
    .unwrap();

    assert_eq!((report.imported, report.known), (2, 0), "{report:?}");
    assert_eq!(steps, ["planned 2", "finished true", "finished true"]);
    assert!(
        report.pending.is_empty(),
        "a database without opencode's queue has nothing pending"
    );
    assert_eq!(store.transcript("ses_a").unwrap().len(), 1, "written whole this time");

    let again = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {
        panic!("nothing left to report");
    })
    .unwrap();

    assert_eq!(again.known, 2);
}

#[test]
fn a_conversation_with_prompts_opencode_queued_but_never_ran_is_named() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    session(&conn, "ses_b", None, "C:/repo", None);
    conn.execute_batch(
        "CREATE TABLE session_input(
             id TEXT PRIMARY KEY, session_id TEXT, prompt TEXT, delivery TEXT,
             admitted_seq INTEGER, promoted_seq INTEGER, time_created INTEGER
         );
         INSERT INTO session_input VALUES('i1', 'ses_a', '{}', 'queue', 1, NULL, 1),
                                        ('i2', 'ses_b', '{}', 'queue', 1, 2, 1);",
    )
    .unwrap();
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!(
        report.pending,
        ["Title ses_a"],
        "a prompt that ran is in the transcript; one that never ran is named"
    );
}

#[test]
fn directories_compare_without_case_slash_style_or_a_trailing_slash() {
    assert_eq!(
        directory_key("C:\\Users\\Kyle\\Repo\\"),
        directory_key("c:/users/kyle/repo")
    );
    assert_eq!(directory_key("C:\\"), "c:/");
    assert_eq!(directory_key("C:/"), "c:/");
    assert_eq!(directory_key("/"), "/");
    assert_ne!(directory_key("C:/repo"), directory_key("C:/repo2"));
}
