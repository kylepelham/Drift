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

    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(undone.session.revert.as_ref().unwrap().message_id, second);
    assert!(undone.kept.is_empty());
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None), "back to before the second prompt");

    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(read(&h, "a.txt"), None, "back to before anything was written");

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "moving the point forward redoes the first turn");

    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(redone.session.revert.is_none() && redone.kept.is_empty());
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")), "redo returns everything");
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().len(), 7, "nothing was deleted along the way");
}

#[tokio::test]
async fn every_blob_undo_needs_is_kept_through_a_prune() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let kept = h.engine.store.recorded_blobs(&h.session.workspace_id).unwrap();
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    let current = h.engine.snapshots.current(&workspace, "a.txt").await.unwrap().unwrap();
    assert!(kept.contains(&current), "the blob a redo would restore is referenced");
    assert_eq!(kept.len(), 3, "one, two and bee, each once; a file that did not exist has no blob");
    h.engine.prune_snapshots().await;
    h.engine.revert(&h.session.id, &second).await.unwrap();
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));
}

#[tokio::test]
async fn undo_leaves_the_users_own_work_alone() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/notes.txt"), "mine\n").unwrap();
    let untouched = std::fs::metadata(h._dir.join("ws/notes.txt")).unwrap().modified().unwrap();
    let (_, second) = two_writing_turns(&h).await;
    // After the session's work: the user edits a file the session never touched and creates another.
    std::fs::write(h._dir.join("ws/notes.txt"), "mine, edited\n").unwrap();
    std::fs::write(h._dir.join("ws/fresh.txt"), "new idea\n").unwrap();
    let edited = std::fs::metadata(h._dir.join("ws/notes.txt")).unwrap().modified().unwrap();
    assert_ne!(untouched, edited);

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(read(&h, "notes.txt").as_deref(), Some("mine, edited\n"), "an unrelated edit survives the undo");
    assert_eq!(read(&h, "fresh.txt").as_deref(), Some("new idea\n"), "a file the user created survives the undo");
    assert_eq!(std::fs::metadata(h._dir.join("ws/notes.txt")).unwrap().modified().unwrap(), edited, "an unrelated file is not even rewritten");
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "the session's own change is undone");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!((read(&h, "notes.txt").as_deref(), read(&h, "fresh.txt").as_deref()), (Some("mine, edited\n"), Some("new idea\n")));
}

#[tokio::test]
async fn a_file_edited_after_the_session_wrote_it_is_kept_and_reported() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    std::fs::write(h._dir.join("ws/a.txt"), "the user's fix\n").unwrap();
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(undone.kept, ["a.txt"]);
    assert_eq!(undone.session.revert.as_ref().unwrap().kept, ["a.txt"], "the marker carries it for the UI");
    assert_eq!(read(&h, "a.txt").as_deref(), Some("the user's fix\n"), "never overwritten");
    assert_eq!(read(&h, "b.txt"), None, "the rest of the turn is still undone");

    std::fs::write(h._dir.join("ws/b.txt"), "user recreated it\n").unwrap();
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.kept.len(), 2, "redo keeps both files that differ from what the undo left");
    assert_eq!(read(&h, "b.txt").as_deref(), Some("user recreated it\n"));
}

#[tokio::test]
async fn a_shell_commands_changes_are_undone_but_not_what_was_there_before() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    std::fs::write(h._dir.join("ws/existing.txt"), "before\n").unwrap();
    // Valid in bash and PowerShell alike, whichever the machine's shell is.
    h.provider.push(tool_call("bash", r#"{"command": "echo made > made.txt"}"#)).push(text("made it"));
    turn(&h, "make a file").await;
    assert!(read(&h, "made.txt").is_some_and(|text| text.contains("made")));
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert_eq!(read(&h, "made.txt"), None);
    assert_eq!(read(&h, "existing.txt").as_deref(), Some("before\n"));
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
