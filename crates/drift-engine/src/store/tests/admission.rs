use super::*;
use crate::session::types::Visibility;
use crate::store::{NewSession, tests::store};

fn new(title: &str) -> NewSession<'_> {
    NewSession {
        workspace_id: "w",
        parent_id: None,
        visibility: Visibility::Sibling,
        title,
        agent: "build",
        model: None,
    }
}

#[test]
fn admission_is_all_or_nothing() {
    let store = store();
    let session = store.create_session(new("")).unwrap();
    let model = ModelRef {
        provider: "p".into(),
        model: "m".into(),
    };
    let Admitted {
        message,
        parts: rows,
        session: updated,
        ..
    } = store
        .admit_prompt(
            &session.id,
            &model,
            vec![Part::Text { text: "hi".into() }],
            Some(("sub_1", "h1")),
        )
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(updated.model, Some(model.clone()));
    assert_eq!(store.transcript(&session.id).unwrap()[0].info.id, message.id);
    let failed = store.admit_prompt("ses_missing", &model, vec![Part::Text { text: "x".into() }], None);
    assert!(failed.is_err());
    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "the failed admission must leave no message behind");

    let found = store.submission("sub_1").unwrap().unwrap();
    assert_eq!(
        (
            found.session_id.as_str(),
            found.message_id.as_str(),
            found.payload_hash.as_str()
        ),
        (session.id.as_str(), message.id.as_str(), "h1")
    );
    assert!(store.submission("sub_nope").unwrap().is_none());
    assert!(store.delete_session(&session.id).unwrap());
    assert!(store.submission("sub_1").unwrap().is_none(), "cascade");
    assert!(store.transcript(&session.id).unwrap().is_empty());
    assert!(!store.delete_session(&session.id).unwrap());
}

#[test]
fn a_reused_submission_id_is_settled_inside_the_admission() {
    let store = store();
    let (first, other) = (
        store.create_session(new("a")).unwrap(),
        store.create_session(new("b")).unwrap(),
    );
    let model = ModelRef {
        provider: "p".into(),
        model: "m".into(),
    };
    let admit = |session: &str, hash| {
        store
            .admit_delivering(
                session,
                Admission {
                    pick: Pick::model(&model),
                    parts: vec![Part::Text { text: "hi".into() }],
                    submission: Some(("sub_1", hash)),
                    handover: Handover::default(),
                },
            )
            .unwrap()
    };

    let Admit::New(landed) = admit(&first.id, "h1") else {
        panic!("first admission")
    };
    assert!(matches!(admit(&first.id, "h1"), Admit::Replayed { message_id } if message_id == landed.message.id));
    assert!(matches!(admit(&first.id, "h2"), Admit::Conflict), "a different prompt");
    assert!(matches!(admit(&other.id, "h1"), Admit::Conflict), "another session");
    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "only the first wrote anything");
}
