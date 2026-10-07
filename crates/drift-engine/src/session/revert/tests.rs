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
async fn an_undo_that_keeps_files_moves_only_the_conversation_and_a_later_one_starts_from_where_they_stand() {
    let h = harness().await;
    let (first, second) = two_writing_turns(&h).await;
    let kept = h.engine.revert_keeping_files(&h.session.id, &first).await.unwrap();
    assert_eq!(kept.session.revert.as_ref().unwrap().message_id, first, "the conversation goes back");
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")), "the files stay");

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None), "an ordinary undo puts back from where the files stood");
    h.engine.revert_keeping_files(&h.session.id, &first).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"), "still kept as they are");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")), "redo returns the files the earlier undo put back");

    h.engine.revert_keeping_files(&h.session.id, &second).await.unwrap();
    h.provider.push(text("carried on"));
    turn(&h, "again").await;
    let prompts = h.engine.store.transcript(&h.session.id).unwrap().iter().filter(|m| m.info.role == Role::User).count();
    assert_eq!(prompts, 2, "the next prompt commits the undo");
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()), (Some("two"), Some("bee")), "and the dropped turns' files stay");
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
async fn an_undo_takes_every_files_turn_before_changing_any() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let ws = crate::tool::canonical(&h._dir.join("ws"));
    let held = crate::tool::lock::files(&[ws.join("b.txt")]).await;
    let (engine, id) = (h.engine.clone(), h.session.id.clone());
    let undo = tokio::spawn(async move { engine.revert(&id, &second).await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!undo.is_finished());
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"), "a.txt waits too, so a rollback never lands over another writer");
    drop(held);
    tokio::time::timeout(Duration::from_secs(5), undo).await.unwrap().unwrap().unwrap();
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None));
}

#[tokio::test]
async fn rollback_keeps_competing_writers_out_until_the_marker_failure_is_repaired() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    let shifted = h.engine.shift(&session, &second, None, Direction::Back).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"));
    let workspace = h._dir.join("ws");
    let competing = tokio::spawn(async move {
        let file = workspace.join("a.txt");
        let _held = crate::tool::lock::files(std::slice::from_ref(&file)).await;
        tokio::fs::write(file, "another session").await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!competing.is_finished(), "undo still holds the files while its marker is uncommitted");
    refuse_marker(&h);
    let marker = Revert::new(&second, Vec::new(), Some(&second));
    assert!(h.engine.mark_or_put_back(&h.session.id, Some(&marker), shifted).await.is_err());
    tokio::time::timeout(Duration::from_secs(5), competing).await.unwrap().unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("another session"), "rollback completed before the competing writer ran");
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());
}

#[tokio::test]
async fn undo_and_redo_merge_one_files_history_across_nested_workspace_moves() {
    let (h, first, second, file) = overlapping_writes(false).await;
    let undone = h.engine.revert(&h.session.id, &first).await.unwrap();
    assert!(undone.kept.is_empty(), "the uninterrupted A -> B -> C chain belongs to this session");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    h.engine.prune_snapshots().await;
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(redone.kept.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "B");
    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "B");
    h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
}

#[tokio::test]
async fn cross_workspace_undo_rolls_back_from_the_endpoint_that_owns_the_previous_bytes() {
    let (h, first, _, file) = overlapping_writes(false).await;
    refuse_marker(&h);
    assert!(matches!(h.engine.revert(&h.session.id, &first).await, Err(RevertError::Files(_))));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C", "C is stored only in the nested workspace's snapshot repository");
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());
    allow_marker(&h);
    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    refuse_marker(&h);
    assert!(matches!(h.engine.unrevert(&h.session.id).await, Err(RevertError::Files(_))));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A", "redo rollback reads A from the original workspace's repository");
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_some());
}

#[tokio::test]
async fn a_broken_cross_workspace_chain_preserves_the_entire_file() {
    let (h, first, _, file) = overlapping_writes(true).await;
    let undone = h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(undone.kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C", "A -> B then external X -> C is not partially undone to X");
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert_eq!(redone.kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
}

async fn overlapping_writes(break_chain: bool) -> (Harness, String, String, PathBuf) {
    let h = harness().await;
    allow_writes(&h);
    let nested = h._dir.join("ws/sub");
    std::fs::create_dir_all(&nested).unwrap();
    let file = nested.join("a.txt");
    std::fs::write(&file, "A").unwrap();
    h.provider.push(tool_call("read", r#"{"path":"sub/a.txt"}"#)).push(text("read"));
    turn(&h, "read the file").await;
    h.provider.push(write("sub/a.txt", "B")).push(text("first write"));
    let first = h.engine.submit(&h.session.id, prompt("first write")).await.unwrap().message.id;
    until_idle(&h).await;
    let workspace = h.engine.store.add_workspace(&nested.to_string_lossy(), "nested", "").unwrap();
    h.engine.move_session(&h.session.id, &workspace.id).unwrap();
    if break_chain { std::fs::write(&file, "X").unwrap(); }
    h.provider.push(write("a.txt", "C")).push(text("second write"));
    let second = h.engine.submit(&h.session.id, prompt("second write")).await.unwrap().message.id;
    until_idle(&h).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");
    (h, first, second, file)
}

#[tokio::test]
async fn undo_deduplicates_paths_across_overlapping_historical_workspaces() {
    let h = harness().await;
    let root = crate::tool::canonical(&h._dir.join("ws"));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let nested = h.engine.store.add_workspace(&root.join("sub").to_string_lossy(), "nested", "").unwrap();
    let change = |owner: String, path: &str| {
        let file = crate::tool::canonical(&h.engine.root_of(&owner).unwrap().join(path));
        Net::new(owner, FileChange { path: path.into(), before: None, after: None, observed: false }, Some(file))
    };
    let nets = [change(h.session.workspace_id.clone(), "sub/a.txt"), change(nested.id, "a.txt")];
    let held = tokio::time::timeout(Duration::from_secs(1), h.engine.turns_for(&nets)).await.expect("one reservation for the same physical file");
    assert!(tokio::time::timeout(Duration::from_millis(50), crate::tool::lock::files(&[root.join("sub/a.txt")])).await.is_err());
    drop(held);
}

#[tokio::test]
async fn stopping_an_undo_waiting_for_files_leaves_them_unchanged() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let held = crate::tool::lock::files(&[h._dir.join("ws/a.txt")]).await;
    let (engine, id) = (h.engine.clone(), h.session.id.clone());
    let undo = tokio::spawn(async move { engine.revert(&id, &second).await });
    for _ in 0..100 {
        if h.engine.turns.is_running(&h.session.id) { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(h.engine.abort(&h.session.id));
    assert!(matches!(tokio::time::timeout(Duration::from_secs(2), undo).await.unwrap().unwrap(), Err(RevertError::Stopped)));
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());
    drop(held);
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

/// Bounded by the wall clock: the paused clock races ahead while the prune waits on its git child.
async fn until_gone(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while path.exists() {
        assert!(std::time::Instant::now() < deadline, "{} was never pruned", path.display());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
async fn undo_refuses_non_prompts_and_stops_a_running_turn_before_undoing() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let reply = h.engine.store.transcript(&h.session.id).unwrap()[1].info.id.clone();
    assert!(matches!(h.engine.revert(&h.session.id, &reply).await, Err(RevertError::NotAPrompt)));

    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1" } else { "sleep 10" };
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    let undone = h.engine.revert(&h.session.id, &second).await.expect("the turn is stopped, then the undo runs");
    assert!(started.elapsed() < Duration::from_secs(5), "it did not wait out the command");
    assert_eq!(undone.session.revert.unwrap().message_id, second);
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None));
    assert!(!h.engine.turns.is_running(&h.session.id));
}

#[tokio::test]
async fn undo_stops_an_mcp_call_under_way_instead_of_waiting_for_it() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/echo-server.cjs");
    let config = crate::mcp::ServerConfig::Stdio { command: "node".into(), args: vec![script.into()], env: Default::default(), cwd: None, timeout_seconds: None };
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine.connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws")))).await.unwrap();
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "mcp".into(), pattern: "*".into(), decision: Decision::Allow }] });
    h.provider.push(tool_call("echo_shout", &json!({ "text": "hang" }).to_string()));
    h.engine.submit(&h.session.id, prompt("hang")).await.unwrap();
    until_running_call(&h).await;
    let started = std::time::Instant::now();
    let undone = h.engine.revert(&h.session.id, &second).await.expect("the call is stopped, then the undo runs");
    assert!(started.elapsed() < Duration::from_secs(2), "it waited {:?} for the call", started.elapsed());
    assert_eq!(undone.session.revert.unwrap().message_id, second);
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
