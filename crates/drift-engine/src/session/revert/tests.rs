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
async fn an_undo_whose_write_fails_once_begun_leaves_the_file_whole_and_can_be_tried_again() {
    use crate::tool::stage::tests::{inject, leftovers, Fault};
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    inject(Fault::AfterStaging, &workspace.join("a.txt"));
    assert!(matches!(h.engine.revert(&h.session.id, &second).await, Err(RevertError::Files(_))));
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"), "not cut short");
    assert!(leftovers(&workspace).is_empty() && h.engine.store.replacements().unwrap().is_empty());
    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "the conflict check still lets the untouched file go back");
}

#[tokio::test]
async fn an_undo_that_fails_partway_puts_back_what_it_already_changed() {
    use crate::tool::stage::tests::{inject, Fault};
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(write("a.txt", "one")).push(write("b.txt", "uno")).push(text("first"));
    turn(&h, "first").await;
    h.provider.push(write("a.txt", "two")).push(write("b.txt", "dos")).push(text("second"));
    turn(&h, "second").await;
    let second = h.engine.store.transcript(&h.session.id).unwrap().iter().filter(|m| m.info.role == Role::User).nth(1).unwrap().info.id.clone();
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    // a.txt goes back first; b.txt's write then fails, as a file held open without delete sharing would.
    inject(Fault::AfterStaging, &workspace.join("b.txt"));
    let Err(RevertError::Files(message)) = h.engine.revert(&h.session.id, &second).await else { panic!("the undo should fail") };
    assert!(message.contains("b.txt") && message.contains("no file was changed"), "{message}");
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("dos")), "not half undone");
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());

    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert!(undone.kept.is_empty(), "nothing the user did not touch is reported as kept: {:?}", undone.kept);
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("one"), Some("uno")));
}

#[tokio::test]
async fn undo_follows_the_order_writes_finished_not_the_order_their_messages_began() {
    let h = harness().await;
    let ws = crate::tool::canonical(&h._dir.join("ws"));
    h.engine.snapshots.bind(&h.session.workspace_id, &ws);
    let mut blobs = Vec::new();
    for content in ["ORIGINAL", "FROM_B", "FROM_A"] {
        std::fs::write(ws.join("shared.txt"), content).unwrap();
        blobs.push(h.engine.snapshots.record(&ws, "shared.txt").await.unwrap());
    }
    let model = crate::session::turn::tests::model();
    let prompt = h.engine.store.admit_prompt(&h.session.id, &model, vec![Part::Text { text: "both".into() }], None).unwrap().message.id;
    // A's message began first; B wrote first, then A wrote over what B left.
    let (a, b) = (h.engine.store.create_reply(&h.session.id, &model, "build").unwrap(), h.engine.store.create_reply(&h.session.id, &model, "build").unwrap());
    let (b_at, a_at) = (crate::id::new("chg"), crate::id::new("chg"));
    let call = |at: &str, before: &Option<String>, after: &Option<String>| Part::ToolCall {
        call_id: crate::id::new("call"),
        name: "write".into(),
        input: json!({}),
        status: crate::session::types::ToolStatus::Done,
        title: None,
        output: None,
        metadata: Some(json!({ "changes": [{ "path": "shared.txt", "before": before, "after": after }], "owner": h.session.workspace_id, "at": at })),
        started_at: None,
        finished_at: None,
    };
    h.engine.store.add_part(&a.id, &h.session.id, call(&a_at, &blobs[1], &blobs[2])).unwrap();
    h.engine.store.add_part(&b.id, &h.session.id, call(&b_at, &blobs[0], &blobs[1])).unwrap();
    let undone = h.engine.revert(&h.session.id, &prompt).await.unwrap();
    assert!(undone.kept.is_empty(), "an unbroken chain of the session's own writes: {:?}", undone.kept);
    assert_eq!(read(&h, "shared.txt").as_deref(), Some("ORIGINAL"));
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "shared.txt").as_deref(), Some("FROM_A"));
}

/// Makes every save of the session's undo point fail, as a database error would, until `allow` is called.
fn refuse_marker(h: &Harness) {
    h.engine.store.lock().execute_batch("CREATE TRIGGER refuse_marker BEFORE UPDATE OF revert_json ON session BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
}

fn allow_marker(h: &Harness) {
    h.engine.store.lock().execute_batch("DROP TRIGGER refuse_marker;").unwrap();
}

#[tokio::test]
async fn an_undo_or_redo_whose_marker_cannot_be_saved_puts_the_files_back_and_can_be_tried_again() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    refuse_marker(&h);
    let Err(RevertError::Files(message)) = h.engine.revert(&h.session.id, &second).await else { panic!("the undo should fail") };
    assert!(message.contains("undo point") && message.contains("no file was changed"), "{message}");
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")), "files and history still agree");
    allow_marker(&h);
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert!(undone.kept.is_empty(), "nothing is wrongly reported as edited elsewhere: {:?}", undone.kept);
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None));

    refuse_marker(&h);
    assert!(matches!(h.engine.unrevert(&h.session.id).await, Err(RevertError::Files(_))));
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None), "a failed redo leaves the undone files");
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_some());
    allow_marker(&h);
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(redone.kept.is_empty() && redone.session.revert.is_none());
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")));
}

#[tokio::test]
async fn every_blob_undo_needs_is_kept_through_a_prune() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let kept = h.engine.store.recorded_blobs().unwrap().remove(&h.session.workspace_id).unwrap();
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    let current = h.engine.snapshots.current(&workspace, "a.txt").await.unwrap().unwrap();
    assert!(kept.contains(&current), "the blob a redo would restore is referenced");
    assert_eq!(kept.len(), 3, "one, two and bee, each once; a file that did not exist has no blob");
    h.engine.prune_snapshots().await;
    h.engine.revert(&h.session.id, &second).await.unwrap();
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));
}

fn old_output(h: &Harness, name: &str) -> std::path::PathBuf {
    let path = h.engine.data_dir.join("tool-output").join("ses_old").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60)).unwrap();
    path
}

async fn until_gone(path: &std::path::Path) {
    for _ in 0..500 {
        if !path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} was never pruned", path.display());
}

#[tokio::test]
async fn housekeeping_runs_again_and_again_and_keeps_what_undo_needs() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let first_old = old_output(&h, "first.log");
    let fresh = h.engine.data_dir.join("tool-output").join("ses_new").join("fresh.log");
    std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    std::fs::write(&fresh, "recent").unwrap();
    tokio::time::pause();
    let maintaining = tokio::spawn(h.engine.clone().maintain());
    until_gone(&first_old).await;
    assert!(fresh.exists(), "recent output is kept");
    let second_old = old_output(&h, "second.log");
    tokio::time::sleep(crate::MAINTENANCE_INTERVAL).await;
    until_gone(&second_old).await;
    tokio::time::resume();
    maintaining.abort();
    h.engine.revert(&h.session.id, &second).await.unwrap();
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"), "pruning kept every blob undo and redo need");
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

/// Waits until a tool call is running: its before-state has been captured and it has started.
async fn until_running_call(h: &Harness) {
    for _ in 0..400 {
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        let running = transcript.iter().flat_map(|m| &m.parts).any(|row| matches!(row.part, Part::ToolCall { status: crate::session::types::ToolStatus::Running, .. }));
        if running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no call started");
}

fn allow_shell(h: &Harness) {
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
}

#[tokio::test]
async fn a_shell_commands_changes_are_reported_but_never_undone() {
    let h = harness().await;
    // A redirection that writes a file is allowed only by a rule naming the whole line.
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "echo made > made.txt".into(), decision: Decision::Allow }] });
    std::fs::write(h._dir.join("ws/existing.txt"), "before\n").unwrap();
    // Valid in bash and PowerShell alike, whichever the machine's shell is.
    h.provider.push(tool_call("bash", r#"{"command": "echo made > made.txt"}"#)).push(text("made it"));
    turn(&h, "make a file").await;
    assert!(read(&h, "made.txt").is_some_and(|text| text.contains("made")));
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert_eq!(undone.unattributed, ["made.txt"], "seen changing while the command ran, so not provably the session's");
    assert!(read(&h, "made.txt").is_some(), "left as it is");
    assert_eq!(read(&h, "existing.txt").as_deref(), Some("before\n"));
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.unattributed, ["made.txt"]);
}

#[tokio::test]
async fn a_user_edit_made_while_a_command_runs_survives_undo() {
    let h = harness().await;
    allow_shell(&h);
    std::fs::write(h._dir.join("ws/mine.txt"), "draft\n").unwrap();
    h.provider.push(tool_call("bash", r#"{"command": "sleep 1"}"#)).push(text("waited"));
    h.engine.submit(&h.session.id, prompt("wait a second")).await.unwrap();
    until_running_call(&h).await;
    std::fs::write(h._dir.join("ws/mine.txt"), "the user's edit\n").unwrap();
    until_idle(&h).await;
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert_eq!(read(&h, "mine.txt").as_deref(), Some("the user's edit\n"), "an edit made during the command is not the session's to undo");
    assert_eq!(undone.unattributed, ["mine.txt"]);
    assert!(undone.kept.is_empty());
}

#[tokio::test]
async fn a_file_the_session_wrote_and_a_command_then_touched_is_left_alone() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![
            Rule { kind: "edit".into(), pattern: "*".into(), decision: Decision::Allow },
            Rule { kind: "bash".into(), pattern: "echo more >> a.txt".into(), decision: Decision::Allow },
        ],
    });
    h.provider.push(write("a.txt", "one")).push(tool_call("bash", r#"{"command": "echo more >> a.txt"}"#)).push(text("done"));
    turn(&h, "write then append").await;
    let after = read(&h, "a.txt").unwrap();
    let prompt_id = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &prompt_id).await.unwrap();
    assert_eq!(read(&h, "a.txt"), Some(after), "part of its history is unattributed, so none of it is applied");
    assert_eq!(undone.unattributed, ["a.txt"]);
}

#[tokio::test]
async fn a_user_edit_between_two_session_writes_survives_undo_and_redo() {
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(write("a.txt", "one")).push(text("wrote one"));
    turn(&h, "first").await;
    std::fs::write(h._dir.join("ws/a.txt"), "the user's line\n").unwrap();
    h.provider.push(tool_call("read", r#"{"path": "a.txt"}"#)).push(write("a.txt", "two")).push(text("wrote two"));
    turn(&h, "second").await;
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));

    let first = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(undone.kept, ["a.txt"], "the chain broke at the user's edit");
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"), "not rolled back past the user's edit");
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.kept, ["a.txt"]);
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));

    let second = h.engine.store.transcript(&h.session.id).unwrap().iter().filter(|m| m.info.role == Role::User).nth(1).unwrap().info.id.clone();
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert!(undone.kept.is_empty(), "within one unbroken change, undo still works");
    assert_eq!(read(&h, "a.txt").as_deref(), Some("the user's line\n"), "back to where the user left it");
}

#[tokio::test]
async fn undo_after_a_move_changes_the_files_where_they_were_written() {
    let h = harness().await;
    allow_writes(&h);
    h.provider.push(write("a.txt", "written in A")).push(text("done"));
    turn(&h, "write it").await;
    let elsewhere = h._dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("a.txt"), "B's own file\n").unwrap();
    let b = h.engine.store.add_workspace(&elsewhere.to_string_lossy(), "B", "").unwrap();
    h.engine.move_session(&h.session.id, &b.id).unwrap();
    h.engine.prune_snapshots().await;

    let first = h.engine.store.transcript(&h.session.id).unwrap()[0].info.id.clone();
    let undone = h.engine.revert(&h.session.id, &first).await.expect("no error after the move");
    assert!(undone.kept.is_empty(), "{:?}", undone.kept);
    assert_eq!(read(&h, "a.txt"), None, "undone in A, where it was written");
    assert_eq!(std::fs::read_to_string(elsewhere.join("a.txt")).unwrap(), "B's own file\n", "B is untouched");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("written in A"), "the blob survived the prune after the move");
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

    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
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
