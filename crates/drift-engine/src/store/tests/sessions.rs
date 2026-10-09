use super::*;
use crate::session::types::{Part, Visibility};
use crate::store::tests::store;

fn new(workspace: &str) -> NewSession<'_> {
    NewSession {
        workspace_id: workspace,
        parent_id: None,
        visibility: Visibility::Sibling,
        title: "",
        agent: "build",
        model: None,
    }
}

#[test]
fn create_list_update_archive() {
    let store = store();
    let first = store.create_session(new("w1")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = store.create_session(new("w1")).unwrap();
    store.create_session(new("w2")).unwrap();

    let listed = store
        .sessions(SessionFilter {
            workspace_id: Some("w1"),
            archived: false,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert_eq!(
        listed.iter().map(|session| &session.id).collect::<Vec<_>>(),
        [&second.id, &first.id]
    );
    let model = ModelRef {
        provider: "anthropic".into(),
        model: "claude".into(),
    };
    let updated = store
        .update_session(&first.id, Some("Title"), Some(&model), Some("plan"))
        .unwrap()
        .unwrap();
    assert_eq!(updated.title, "Title");
    assert_eq!(updated.model, Some(model));
    assert_eq!(updated.agent, "plan");

    store.set_session_archived(&second.id, true).unwrap();
    let active = store
        .sessions(SessionFilter {
            workspace_id: Some("w1"),
            archived: false,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert_eq!(active.len(), 1);
    let archived = store
        .sessions(SessionFilter {
            workspace_id: None,
            archived: true,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert_eq!(archived[0].id, second.id);
}

#[test]
fn subagents_are_listed_with_their_parent() {
    let store = store();
    let parent = store.create_session(new("w")).unwrap();
    let child = store
        .create_session(NewSession {
            parent_id: Some(&parent.id),
            visibility: Visibility::Hidden,
            ..new("w")
        })
        .unwrap();

    let listed = store
        .sessions(SessionFilter {
            workspace_id: Some("w"),
            archived: false,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|session| session.id == child.id && session.parent_id.as_deref() == Some(parent.id.as_str()))
    );
}

#[test]
fn equal_timestamps_do_not_skip_sessions_across_pages() {
    let store = store();
    let ids: Vec<String> = (0..5).map(|_| store.create_session(new("w")).unwrap().id).collect();
    store
        .lock()
        .execute("UPDATE session SET updated_at = 1000", [])
        .unwrap();
    let mut seen = Vec::new();
    let mut before: Option<String> = None;

    loop {
        let page = store
            .sessions(SessionFilter {
                workspace_id: Some("w"),
                archived: false,
                before: before.as_deref(),
                limit: 2,
            })
            .unwrap();
        seen.extend(page.iter().map(|session| session.id.clone()));
        if page.len() < 2 {
            break;
        }
        before = page.last().map(|session| session.id.clone());
    }

    let mut expected = ids.clone();
    expected.sort();
    expected.reverse();
    assert_eq!(seen, expected);
}

#[test]
fn a_purge_takes_the_sessions_subagents_but_leaves_its_threads() {
    let store = store();
    let root = store.create_session(new("w")).unwrap();
    let child = store
        .create_session(NewSession {
            parent_id: Some(&root.id),
            visibility: Visibility::Hidden,
            ..new("w")
        })
        .unwrap();
    let grandchild = store
        .create_session(NewSession {
            parent_id: Some(&child.id),
            visibility: Visibility::Hidden,
            ..new("w")
        })
        .unwrap();
    let thread = store
        .create_session(NewSession {
            parent_id: Some(&root.id),
            visibility: Visibility::Sibling,
            ..new("w")
        })
        .unwrap();
    let model = ModelRef {
        provider: "p".into(),
        model: "m".into(),
    };
    store
        .admit_prompt(&grandchild.id, &model, vec![Part::Text { text: "deep".into() }], None)
        .unwrap();

    assert_eq!(
        store.purge_archived(&root.id).unwrap(),
        Purge::Active,
        "not archived yet"
    );
    store.set_session_archived(&root.id, true).unwrap();
    assert_eq!(store.purge_archived(&root.id).unwrap(), Purge::Deleted);
    for gone in [&root.id, &child.id, &grandchild.id] {
        assert!(store.session(gone).unwrap().is_none(), "{gone} left behind");
    }
    assert!(
        store.session(&thread.id).unwrap().is_some(),
        "a spawned thread is its own conversation"
    );
    let parts: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM part", [], |row| row.get(0))
        .unwrap();
    assert_eq!(parts, 0, "the subagents' transcripts went with them");
}
