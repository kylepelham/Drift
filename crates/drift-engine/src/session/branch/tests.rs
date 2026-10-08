use std::time::Duration;

use serde_json::json;

use super::*;
use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::tests::{Harness, harness, prompt, text, tool_call, until_idle};
use crate::session::types::Role;

async fn conversation(h: &Harness) {
    h.provider.push(text("Parser tidied"));
    h.engine.submit(&h.session.id, prompt("tidy the parser")).await.unwrap();
    until_idle(h).await;
}

async fn until_session_idle(h: &Harness, id: &str) {
    for _ in 0..300 {
        if !h.engine.turns.is_running(id) {
            return;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} never finished");
}

#[tokio::test]
async fn a_spawn_starts_at_once_with_the_conversation_and_the_instruction() {
    let h = harness().await;
    conversation(&h).await;

    let cutoff = h.engine.store.transcript(&h.session.id).unwrap()[1].info.id.clone();
    h.engine.store.set_session_variant(&h.session.id, Some("high")).unwrap();

    h.provider.push(text("Investigating"));

    let spawned = h
        .engine
        .spawn(&h.session.id, "Investigate why I am getting major fps loss")
        .await
        .unwrap();
    until_session_idle(&h, &spawned.id).await;

    let stored = h.engine.store.session(&spawned.id).unwrap().unwrap();
    assert_eq!(
        (stored.parent_id.as_deref(), stored.visibility),
        (Some(h.session.id.as_str()), Visibility::Sibling),
        "linked to its source"
    );
    assert_eq!(stored.branch_cutoff.as_deref(), Some(cutoff.as_str()));
    assert_eq!(
        stored.variant.as_deref(),
        Some("high"),
        "it thinks at its source's level"
    );
    assert_eq!(stored.title, "Investigate why I am getting major");

    let requests = h.provider.requests.lock().unwrap().clone();
    let sent = format!("{:?}", requests.last().unwrap().messages);
    assert!(
        sent.contains("tidy the parser") && sent.contains("Parser tidied"),
        "the model sees the source conversation: {sent}"
    );
    assert!(
        sent.contains("fps loss") && sent.contains("new thread spawned from the conversation above"),
        "the model is told it is a new thread: {sent}"
    );
    assert_eq!(
        requests.len(),
        2,
        "no drafting request: one for the source, one for the spawn"
    );

    let transcript = h.engine.store.transcript(&spawned.id).unwrap();
    assert_eq!(
        transcript.iter().filter(|m| m.info.role == Role::User).count(),
        2,
        "the copied prompt and the instruction"
    );
    let copied: Vec<bool> = transcript.iter().map(|m| is_copied(&stored, &m.info)).collect();
    assert_eq!(
        copied,
        [true, true, false, false],
        "the copy is older than the thread; its own prompt and reply are not"
    );
    let own = transcript
        .iter()
        .find(|m| m.info.role == Role::User && !is_copied(&stored, &m.info))
        .unwrap();
    assert_eq!(
        format!("{:?}", own.parts).matches("Text").count(),
        1,
        "only the instruction is stored, as typed"
    );
    assert!(!format!("{:?}", own.parts).contains("spawned"));
}

#[tokio::test]
async fn a_fork_is_not_framed_as_a_spawned_thread() {
    let h = harness().await;
    conversation(&h).await;

    let fork = h.engine.fork(&h.session.id, None).unwrap();
    let mut transcript = h.engine.store.transcript(&fork.id).unwrap();
    assert!(transcript.iter().all(|m| !is_copied(&fork, &m.info)));

    let before = transcript.clone();
    frame_spawned(&fork, &mut transcript);

    assert_eq!(transcript, before);
}

#[tokio::test]
async fn a_conversation_with_nothing_finished_spawns_with_just_the_instruction() {
    let h = harness().await;
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&crate::session::turn::tests::model()), None)
        .unwrap();

    h.provider.push(text("On it"));
    let spawned = h.engine.spawn(&h.session.id, "start fresh").await.unwrap();
    until_session_idle(&h, &spawned.id).await;

    assert_eq!(
        h.engine.store.session(&spawned.id).unwrap().unwrap().branch_cutoff,
        None
    );
    assert_eq!(h.engine.store.transcript(&spawned.id).unwrap().len(), 2);
}

#[tokio::test]
async fn stopping_the_source_does_not_stop_what_it_spawned() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "bash".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
    });

    let sleep = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };

    h.provider
        .push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.unwrap();
    h.provider
        .push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    let spawned = h.engine.spawn(&h.session.id, "wait elsewhere").await.unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;

    assert!(
        h.engine.turns.is_running(&spawned.id),
        "a spawned thread is not a worker of its source"
    );

    assert!(h.engine.abort(&spawned.id));
    until_session_idle(&h, &spawned.id).await;
}

#[tokio::test]
async fn subagents_and_empty_instructions_are_refused() {
    let h = harness().await;
    conversation(&h).await;

    let subagent = h
        .engine
        .store
        .create_session(NewSession {
            workspace_id: &h.session.workspace_id,
            parent_id: Some(&h.session.id),
            visibility: Visibility::Hidden,
            title: "",
            agent: "build",
            model: None,
        })
        .unwrap();

    assert!(matches!(
        h.engine.spawn(&subagent.id, "anything").await,
        Err(BranchError::FromSubagent)
    ));
    assert!(matches!(
        h.engine.spawn(&h.session.id, "  ").await,
        Err(BranchError::EmptyInstruction)
    ));

    let count: i64 = h
        .engine
        .store
        .lock()
        .query_row("SELECT COUNT(*) FROM session WHERE visibility = 'sibling'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1, "refused spawns leave nothing behind");
}
