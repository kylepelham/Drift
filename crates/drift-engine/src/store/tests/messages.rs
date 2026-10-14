use super::*;
use crate::session::types::Visibility;
use crate::store::{NewSession, tests::store};

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
fn messages_page_backwards_with_parts() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let mut ids = Vec::new();
    for index in 0..5 {
        let message = store.create_message(&session.id, Role::User, None).unwrap();
        store
            .add_part(
                &message.id,
                &session.id,
                Part::Text {
                    text: format!("m{index}"),
                },
            )
            .unwrap();
        ids.push(message.id);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let page = store.messages(&session.id, None, 2).unwrap();
    assert_eq!(
        page.iter().map(|message| &message.info.id).collect::<Vec<_>>(),
        [&ids[3], &ids[4]]
    );
    let older = store.messages(&session.id, Some(&ids[3]), 2).unwrap();
    assert_eq!(
        older.iter().map(|message| &message.info.id).collect::<Vec<_>>(),
        [&ids[1], &ids[2]]
    );
    assert_eq!(older[0].parts[0].part, Part::Text { text: "m1".into() });
    assert_eq!(store.transcript(&session.id).unwrap().len(), 5);
}

#[test]
fn a_part_this_build_cannot_read_loads_as_unknown_and_is_saved_back_unchanged() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let message = store.create_message(&session.id, Role::User, None).unwrap();
    store
        .add_part(&message.id, &session.id, Part::Text { text: "kept".into() })
        .unwrap();
    let stored = [
        r#"{"type":"patch","hash":"abc","files":["a.rs"]}"#,
        r#"{"type":"unknown","raw":"x"}"#,
        r#"{"type":"text"}"#,
    ];
    for (index, json) in stored.iter().enumerate() {
        store
            .lock()
            .execute(
                "INSERT INTO part(id, message_id, session_id, json) VALUES(?1, ?2, ?3, ?4)",
                params![format!("prt_z{index}"), message.id, session.id, json],
            )
            .unwrap();
    }

    let parts = store.transcript(&session.id).unwrap().remove(0).parts;
    assert_eq!(
        parts[0].part,
        Part::Text { text: "kept".into() },
        "the rest of the conversation still loads"
    );
    let raws: Vec<&str> = parts[1..]
        .iter()
        .map(|row| match &row.part {
            Part::Unknown { raw } => raw.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        raws, stored,
        "each kept as stored, even one that names the unknown type itself"
    );
    for row in &parts[1..] {
        store.save_part(row).unwrap();
    }
    let on_disk: Vec<String> = store
        .lock()
        .prepare("SELECT json FROM part WHERE id LIKE 'prt_z%' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(on_disk, stored, "saving one back writes the same bytes");
    let shown = serde_json::to_value(&parts[1]).unwrap();
    assert_eq!(
        (shown["type"].as_str(), shown["raw"].as_str()),
        (Some("unknown"), Some(stored[0])),
        "a client sees the type and the raw text"
    );
}

#[test]
fn parts_land_on_their_own_messages_and_the_last_reply_loads_alone() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let other = store.create_session(new("w")).unwrap();
    let prompt = store.create_message(&session.id, Role::User, None).unwrap();
    store
        .add_part(&prompt.id, &session.id, Part::Text { text: "ask".into() })
        .unwrap();
    let elsewhere = store.create_message(&other.id, Role::User, None).unwrap();
    store
        .add_part(
            &elsewhere.id,
            &other.id,
            Part::Text {
                text: "not this session".into(),
            },
        )
        .unwrap();
    let empty = store.create_message(&session.id, Role::Assistant, None).unwrap();
    let reply = store.create_message(&session.id, Role::Assistant, None).unwrap();
    store
        .add_part(&reply.id, &session.id, Part::Text { text: "a".into() })
        .unwrap();
    store
        .add_part(&reply.id, &session.id, Part::Text { text: "b".into() })
        .unwrap();

    let transcript = store.transcript(&session.id).unwrap();
    let counts: Vec<usize> = transcript.iter().map(|message| message.parts.len()).collect();
    assert_eq!(
        counts,
        [1, 0, 2],
        "each message has its own parts, in order, and nothing from another session"
    );
    assert_eq!(transcript[2].parts[1].part, Part::Text { text: "b".into() });
    let last = store.last_reply(&session.id).unwrap().unwrap();
    assert_eq!((last.info.id.as_str(), last.parts.len()), (reply.id.as_str(), 2));
    assert_eq!(store.with_parts(&empty.id).unwrap().unwrap().parts.len(), 0);
    assert!(
        store
            .last_reply(&store.create_session(new("w")).unwrap().id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn assistant_messages_stream_then_save() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let mut message = store.create_message(&session.id, Role::Assistant, None).unwrap();
    assert_eq!(message.status, MessageStatus::Streaming);
    message.status = MessageStatus::Done;
    message.usage = Usage {
        input: 10,
        output: 5,
        ..Usage::default()
    };
    message.finished_at = Some(1);
    message.generation_ms = Some(2_500);
    store.save_message(&message).unwrap();
    assert_eq!(store.message(&message.id).unwrap().unwrap(), message);
    assert_eq!(serde_json::to_value(&message).unwrap()["generationMs"], 2_500);

    for ending in [
        crate::session::types::Ending::Length,
        crate::session::types::Ending::Refused,
        crate::session::types::Ending::Limit,
    ] {
        message.ending = Some(ending);
        store.save_message(&message).unwrap();
        assert_eq!(
            store.message(&message.id).unwrap().unwrap().ending,
            Some(ending),
            "every ending is stored"
        );
    }
}

#[test]
fn streaming_messages_are_abandoned_on_open() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    store.create_message(&session.id, Role::Assistant, None).unwrap();

    assert_eq!(store.abandon_streaming_messages().unwrap(), 1);
    assert_eq!(store.abandon_streaming_messages().unwrap(), 0);
}

#[test]
fn measured_generation_survives_forks_and_old_replies_stay_unmeasured() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let old = store.create_message(&session.id, Role::Assistant, None).unwrap();
    assert_eq!(old.generation_ms, None);
    let mut reply = store.create_message(&session.id, Role::Assistant, None).unwrap();
    reply.status = MessageStatus::Done;
    reply.generation_ms = Some(3_000);
    store.save_message(&reply).unwrap();
    let fork = store
        .fork_session(&session.id, new("w"), &reply.id, None)
        .unwrap()
        .unwrap();
    let copied = store.transcript(&fork.id).unwrap();
    assert_eq!(copied.last().unwrap().info.generation_ms, Some(3_000));
}

#[test]
fn the_account_that_sent_a_reply_is_kept_and_copied_into_forks() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let mut reply = store.create_message(&session.id, Role::Assistant, None).unwrap();
    assert_eq!(reply.account, None);

    reply.status = MessageStatus::Done;
    reply.account = Some("openai~a1b2c3".into());
    store.save_message(&reply).unwrap();
    assert_eq!(store.message(&reply.id).unwrap().unwrap().account, reply.account);

    let fork = store
        .fork_session(&session.id, new("w"), &reply.id, None)
        .unwrap()
        .unwrap();
    let copied = store.transcript(&fork.id).unwrap();
    assert_eq!(copied.last().unwrap().info.account, reply.account);
}

#[test]
fn parts_round_trip_through_json() {
    let store = store();
    let session = store.create_session(new("w")).unwrap();
    let message = store.create_message(&session.id, Role::Assistant, None).unwrap();
    let mut row = store
        .add_part(
            &message.id,
            &session.id,
            Part::Reasoning {
                text: "hm".into(),
                signature: Some("sig".into()),
                redacted: None,
            },
        )
        .unwrap();

    row.part = Part::Reasoning {
        text: "hmm".into(),
        signature: Some("sig".into()),
        redacted: None,
    };
    store.save_part(&row).unwrap();
    let loaded = store.transcript(&session.id).unwrap();
    assert_eq!(loaded[0].parts, vec![row]);
}
