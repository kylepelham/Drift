use std::time::Duration;

use serde_json::json;

use super::*;
use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle, Harness};
use crate::session::turn::TurnError;
use crate::session::types::MessageStatus;

fn allow_writes(h: &Harness) {
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow }] });
}

fn write(path: &str, content: &str) -> Vec<crate::llm::Chunk> {
    tool_call("write", &json!({ "path": path, "content": content }).to_string())
}

async fn turn(h: &Harness, ask: &str) {
    h.engine.submit(&h.session.id, prompt(ask)).await.unwrap();
    until_idle(h).await;
}

fn read(h: &Harness, path: &str) -> Option<String> {
    std::fs::read_to_string(h._dir.join("ws").join(path)).ok()
}

/// Two turns: the first writes a.txt, the second rewrites it and adds b.txt. Returns both prompts' ids.
async fn two_writing_turns(h: &Harness) -> (String, String) {
    allow_writes(h);
    h.provider.push(write("a.txt", "one")).push(text("wrote a"));
    turn(h, "first").await;
    h.provider.push(write("a.txt", "two")).push(write("b.txt", "bee")).push(text("rewrote"));
    turn(h, "second").await;
    let prompts: Vec<String> = h.engine.store.transcript(&h.session.id).unwrap().iter().filter(|m| m.info.role == Role::User).map(|m| m.info.id.clone()).collect();
    (prompts[0].clone(), prompts[1].clone())
}

#[tokio::test]
async fn undo_redo_and_moving_the_point_keep_files_and_history_in_step() {
    let h = harness().await;
    let (first, second) = two_writing_turns(&h).await;
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")));

    let session = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(session.revert.as_ref().unwrap().message_id, second);
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None), "back to before the second prompt");

    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(read(&h, "a.txt"), None, "back to before anything was written");

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "moving the point forward redoes the first turn");

    let session = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(session.revert.is_none());
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")), "redo returns everything");
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().len(), 7, "nothing was deleted along the way");
}

#[tokio::test]
async fn a_prompt_sent_while_undone_commits_the_undo() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    h.engine.revert(&h.session.id, &second).await.unwrap();
    let mut rx = h.engine.hub.attach(None).rx;
    h.provider.push(text("fresh start"));
    turn(&h, "different second").await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let texts: Vec<String> = transcript.iter().flat_map(|m| m.parts.iter()).filter_map(|row| match &row.part { Part::Text { text } => Some(text.clone()), _ => None }).collect();
    assert_eq!(texts, ["first", "wrote a", "different second", "fresh start"]);
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "the files stay where the undo left them");
    let mut removed = 0;
    while let Ok(envelope) = rx.try_recv() {
        if matches!(envelope.event, Event::MessageRemoved { .. }) {
            removed += 1;
        }
    }
    assert_eq!(removed, 4, "the hidden prompt and its three replies are announced as gone");
}

#[tokio::test]
async fn undo_refuses_non_prompts_and_running_sessions() {
    let h = harness().await;
    let (first, _) = two_writing_turns(&h).await;
    let reply = h.engine.store.transcript(&h.session.id).unwrap()[1].info.id.clone();
    assert!(matches!(h.engine.revert(&h.session.id, &reply).await, Err(RevertError::NotAPrompt)));

    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1 > nul" } else { "sleep 10" };
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(matches!(h.engine.revert(&h.session.id, &first).await, Err(RevertError::Busy)));
    h.engine.abort(&h.session.id);
    until_idle(&h).await;
}

#[tokio::test]
async fn undo_puts_back_what_a_subagent_wrote() {
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(text("ok"));
    turn(&h, "warm up").await;
    h.provider
        .push(tool_call("task", r#"{"description": "Write c", "prompt": "write c.txt"}"#))
        .push(write("c.txt", "sea"))
        .push(text("child wrote c"))
        .push(text("parent done"));
    turn(&h, "delegate").await;
    assert_eq!(read(&h, "c.txt").as_deref(), Some("sea"));
    let delegated = h.engine.store.transcript(&h.session.id).unwrap().iter().filter(|m| m.info.role == Role::User).nth(1).unwrap().info.id.clone();
    h.engine.revert(&h.session.id, &delegated).await.unwrap();
    assert_eq!(read(&h, "c.txt"), None, "the subagent's write is undone with its parent's prompt");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "c.txt").as_deref(), Some("sea"));
}

#[tokio::test]
async fn while_undone_a_fork_copies_only_what_is_visible_and_compaction_waits() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    h.engine.revert(&h.session.id, &second).await.unwrap();
    let fork = h.engine.fork(&h.session.id, None).unwrap();
    let copied = h.engine.store.transcript(&fork.id).unwrap();
    assert_eq!(copied.len(), 3, "the first prompt, its write and its reply");
    assert!(copied.iter().all(|m| m.info.status == MessageStatus::Done));
    assert_eq!(h.engine.start_compaction(&h.session.id), Err(TurnError::Reverted));
    assert!(!h.engine.turns.is_running(&h.session.id), "the refused compaction left nothing claimed");
}
