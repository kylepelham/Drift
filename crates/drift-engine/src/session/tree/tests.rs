use std::time::Duration;

use serde_json::json;

use super::*;
use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle, Harness};
use crate::session::types::Part;

#[tokio::test]
async fn a_move_is_refused_while_a_turn_is_still_planning() {
    let h = harness().await;
    let other = h.engine.store.add_workspace(&h._dir.join("other").to_string_lossy(), "other", "").unwrap();
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("write", r#"{"path": "out.txt", "content": "x"}"#)).push(text("done"));
    // An expired token sends planning to the refresh lock; holding it keeps the turn in planning.
    h.engine.credentials.set("anthropic", &crate::llm::Credential::OAuth { access: "stale".into(), refresh: "r".into(), expires_at: 1, account: None }).unwrap();
    let lock = h.engine.turns.refresh_lock("anthropic");
    let held = lock.lock().await;
    let (engine, id) = (h.engine.clone(), h.session.id.clone());
    let submitted = tokio::spawn(async move { engine.submit(&id, prompt("write it")).await.map(|_| ()) });
    for _ in 0..200 {
        if h.engine.turns.is_running(&h.session.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(matches!(h.engine.move_session(&h.session.id, &other.id), Err(TreeError::Busy)), "planning holds the session");

    // A usable key appears, so the waiting turn skips the network refresh.
    h.engine.credentials.set("anthropic", &crate::llm::Credential::ApiKey { key: "k".into() }).unwrap();
    drop(held);
    submitted.await.unwrap().unwrap();
    until_idle(&h).await;
    assert!(h._dir.join("ws/out.txt").exists(), "the turn wrote where the session was");
    assert!(h.engine.move_session(&h.session.id, &other.id).is_ok(), "once idle, the move goes through");
}

async fn two_turns(h: &Harness) {
    for (ask, reply) in [("first", "one"), ("second", "two")] {
        h.provider.push(text(reply));
        h.engine.submit(&h.session.id, prompt(ask)).await.unwrap();
        until_idle(h).await;
    }
}

fn texts(h: &Harness, id: &str) -> Vec<String> {
    h.engine
        .store
        .transcript(id)
        .unwrap()
        .iter()
        .flat_map(|m| m.parts.iter().filter_map(|row| match &row.part { Part::Text { text } => Some(text.clone()), _ => None }))
        .collect()
}

fn long_bash(h: &Harness) {
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
}

#[tokio::test]
async fn a_fork_copies_the_history_into_an_independent_conversation() {
    let h = harness().await;
    two_turns(&h).await;
    h.engine.store.update_session(&h.session.id, None, None, Some("plan")).unwrap();
    h.engine.store.set_session_variant(&h.session.id, Some("high")).unwrap();
    let fork = h.engine.fork(&h.session.id, None).unwrap();
    assert_eq!(texts(&h, &fork.id), ["first", "one", "second", "two"]);
    assert_eq!((fork.agent.as_str(), fork.variant.as_deref()), ("plan", Some("high")), "the fork runs as the source does");
    let agents: Vec<Option<String>> = h.engine.store.transcript(&fork.id).unwrap().into_iter().map(|m| m.info.agent).collect();
    assert!(agents.iter().all(|agent| agent.as_deref() == Some("build")), "each copy keeps the agent it ran as: {agents:?}");
    assert_eq!((fork.parent_id.as_deref(), fork.visibility), (None, Visibility::Sibling));
    let source = h.engine.store.transcript(&h.session.id).unwrap();
    let copy = h.engine.store.transcript(&fork.id).unwrap();
    assert!(copy.iter().all(|m| m.info.session_id == fork.id && !source.iter().any(|s| s.info.id == m.info.id)), "copies get their own ids");

    h.provider.push(text("three"));
    h.engine.submit(&fork.id, prompt("third")).await.unwrap();
    for _ in 0..200 {
        if !h.engine.turns.is_running(&fork.id) { break }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(texts(&h, &h.session.id).len(), 4, "the source does not see the fork's new turns");
    let request = h.provider.requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(request.messages.len(), 5, "the fork replays the copied history");
}

#[tokio::test]
async fn a_fork_carries_the_reads_its_copied_history_shows() {
    let h = harness().await;
    for (name, ask) in [("a.txt", "read a"), ("b.txt", "read b")] {
        std::fs::write(h._dir.join("ws").join(name), "x\n").unwrap();
        h.provider.push(tool_call("read", &json!({ "path": name }).to_string())).push(text("read"));
        h.engine.submit(&h.session.id, prompt(ask)).await.unwrap();
        until_idle(&h).await;
    }
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let end_of_first = transcript[transcript.iter().rposition(|m| m.info.role == Role::User).unwrap() - 1].info.id.clone();
    let names = |id: &str| {
        let mut paths: Vec<String> = h.engine.store.read_files(id).unwrap().iter().map(|p| p.rsplit(['/', '\\']).next().unwrap().to_string()).collect();
        paths.sort();
        paths
    };
    assert_eq!(names(&h.engine.fork(&h.session.id, Some(&end_of_first)).unwrap().id), ["a.txt"]);
    assert_eq!(names(&h.engine.fork(&h.session.id, None).unwrap().id), ["a.txt", "b.txt"]);
}

#[tokio::test]
async fn a_bounded_fork_stops_at_the_chosen_message() {
    let h = harness().await;
    two_turns(&h).await;
    let first_reply = h.engine.store.transcript(&h.session.id).unwrap()[1].info.id.clone();
    let fork = h.engine.fork(&h.session.id, Some(&first_reply)).unwrap();
    assert_eq!(texts(&h, &fork.id), ["first", "one"]);
    assert!(matches!(h.engine.fork(&h.session.id, Some("msg_nope")), Err(TreeError::BadMessage)));
}

#[tokio::test]
async fn forking_a_running_session_leaves_the_turn_in_flight_out() {
    let h = harness().await;
    two_turns(&h).await;
    long_bash(&h);
    h.engine.submit(&h.session.id, prompt("wait")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A prompt steered into the running turn moves the last prompt, not where the turn began.
    h.engine.submit(&h.session.id, prompt("and also")).await.unwrap();
    let fork = h.engine.fork(&h.session.id, None).unwrap();
    assert_eq!(texts(&h, &fork.id), ["first", "one", "second", "two"]);
    assert!(h.engine.store.session(&fork.id).unwrap().unwrap().archived_at.is_none(), "a finished fork is listed");
    let in_flight = h.engine.store.transcript(&h.session.id).unwrap().last().unwrap().info.id.clone();
    assert!(matches!(h.engine.fork(&h.session.id, Some(&in_flight)), Err(TreeError::BadMessage)));
    h.engine.abort(&h.session.id);
    until_idle(&h).await;

    let empty = h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None }).unwrap();
    assert!(matches!(h.engine.fork(&empty.id, None), Err(TreeError::Empty)));
}

#[tokio::test]
async fn a_move_takes_subagents_but_not_branches_and_waits_for_idle() {
    let h = harness().await;
    let other = h.engine.store.add_workspace(&h._dir.join("other").to_string_lossy(), "other", "").unwrap();
    let child = |visibility| h.engine.store.create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: Some(&h.session.id), visibility, title: "", agent: "build", model: None }).unwrap();
    let subagent = child(Visibility::Hidden);
    let branch = child(Visibility::Sibling);

    long_bash(&h);
    h.engine.submit(&subagent.id, prompt("wait")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(matches!(h.engine.move_session(&h.session.id, &other.id), Err(TreeError::Busy)), "a running subagent blocks the move");
    h.engine.abort(&subagent.id);
    for _ in 0..200 {
        if !h.engine.turns.is_running(&subagent.id) { break }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let mut moved = h.engine.move_session(&h.session.id, &other.id).unwrap();
    moved.sort();
    let mut expected = vec![h.session.id.clone(), subagent.id.clone()];
    expected.sort();
    assert_eq!(moved, expected);
    let workspace_of = |id: &str| h.engine.store.session(id).unwrap().unwrap().workspace_id;
    assert_eq!(workspace_of(&subagent.id), other.id);
    assert_eq!(workspace_of(&branch.id), h.session.workspace_id, "a branch is its own conversation and stays put");
    assert!(matches!(h.engine.move_session(&h.session.id, "nope"), Err(TreeError::NoWorkspace)));
}
