use std::collections::HashSet;
use std::path::{Path, PathBuf};

use drift_engine::session::types::{Ending, MessageStatus, Part, Role, ToolStatus, Visibility};
use drift_engine::store::Store;
use rusqlite::{params, Connection};
use serde_json::json;

use super::*;

/// Versions kept by their content's hash, so a test can read back what a record names.
#[derive(Default)]
struct Kept(HashMap<String, String>);

impl Blobs for Kept {
    fn store(&mut self, _owner: &str, _root: &Path, bytes: &[u8]) -> Option<String> {
        use sha2::Digest;
        let id = format!("{:x}", sha2::Sha256::digest(bytes));
        self.0.insert(id.clone(), String::from_utf8(bytes.to_vec()).unwrap());
        Some(id)
    }
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir() -> Dir {
    let path = std::env::temp_dir().join(format!("drift-migrate-{}", drift_engine::id::new("t")));
    std::fs::create_dir_all(&path).unwrap();
    Dir(path)
}

/// opencode's tables with the columns the import reads.
fn opencode(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT NOT NULL);
         CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, directory TEXT NOT NULL, title TEXT NOT NULL, agent TEXT, model TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER);
         CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE todo(session_id TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, priority TEXT NOT NULL, position INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);",
    )
    .unwrap();
    conn
}

fn session(conn: &Connection, id: &str, parent: Option<&str>, directory: &str, archived: Option<i64>) {
    let model = json!({ "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" }).to_string();
    conn.execute("INSERT INTO session VALUES(?1, 'p', ?2, ?3, ?4, 'build', ?5, 1000, 9000, ?6)", params![id, parent, directory, format!("Title {id}"), model, archived]).unwrap();
}

fn message(conn: &Connection, session: &str, id: &str, created: i64, data: serde_json::Value, parts: &[serde_json::Value]) {
    conn.execute("INSERT INTO message VALUES(?1, ?2, ?3, ?3, ?4)", params![id, session, created, data.to_string()]).unwrap();
    for (n, part) in parts.iter().enumerate() {
        conn.execute("INSERT INTO part VALUES(?1, ?2, ?3, ?4, ?4, ?5)", params![format!("prt_{id}_{n:02}"), id, session, created, part.to_string()]).unwrap();
    }
}

fn user(created: i64) -> serde_json::Value {
    json!({ "role": "user", "time": { "created": created }, "agent": "build", "model": { "providerID": "anthropic", "modelID": "claude-opus-5-5" } })
}

fn assistant(created: i64, extra: serde_json::Value) -> serde_json::Value {
    let mut data = json!({ "role": "assistant", "agent": "build", "providerID": "anthropic", "modelID": "claude-opus-5-5", "cost": 0.5,
        "tokens": { "input": 10, "output": 20, "reasoning": 5, "cache": { "read": 100, "write": 7 } }, "time": { "created": created, "completed": created + 5 } });
    data.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    data
}

fn store_with(dir: &Path, workspaces: &[&str]) -> Store {
    let store = drift_engine::store::open(&dir.join("drift")).unwrap();
    for path in workspaces {
        store.add_workspace(path, "w", "").unwrap();
    }
    store
}

#[test]
fn a_conversation_arrives_in_its_workspace_with_every_part_in_this_engines_shape() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/Users/Kyle/Repo", None);
    let patch = json!({ "type": "patch", "hash": "abc", "files": ["a.rs"] });
    let nudge = json!({ "type": "text", "synthetic": true, "text": "Continue." });
    message(&conn, "ses_a", "msg_1", 2000, user(2000), &[json!({ "type": "text", "text": "fix it" }), nudge.clone(), json!({ "type": "file", "mime": "text/plain", "filename": "a.rs", "url": "data:text/plain;base64,YQ==", "source": { "type": "file", "path": "src/a.rs" } })]);
    let edit = json!({ "type": "tool", "tool": "edit", "callID": "toolu_1", "state": { "status": "completed", "input": { "filePath": "a.rs", "oldString": "a", "newString": "b" }, "output": "Edit applied successfully.", "title": "a.rs", "metadata": { "diff": "-a\n+b" }, "time": { "start": 2010, "end": 2020 } } });
    let failed = json!({ "type": "tool", "tool": "bash", "callID": "toolu_2", "state": { "status": "error", "input": { "command": "boom" }, "error": "exit 1", "time": { "start": 2021, "end": 2022 } } });
    message(&conn, "ses_a", "msg_2", 2005, assistant(2005, json!({})), &[json!({ "type": "step-start", "snapshot": "x" }), json!({ "type": "reasoning", "text": "think", "metadata": { "anthropic": { "signature": "sig" } } }), edit, failed, patch.clone(), json!({ "type": "step-finish", "reason": "stop" }), json!({ "type": "text", "text": "done" })]);
    message(&conn, "ses_a", "msg_3", 3000, user(3000), &[json!({ "type": "compaction", "auto": true, "tail_start_id": "msg_2" })]);
    message(&conn, "ses_a", "msg_4", 3001, assistant(3001, json!({ "summary": true })), &[json!({ "type": "text", "text": "summary" })]);
    message(&conn, "ses_a", "msg_5", 4000, assistant(4000, json!({ "error": { "name": "MessageAbortedError", "data": { "message": "Aborted" } } })), &[]);
    message(&conn, "ses_a", "msg_6", 5000, assistant(5000, json!({ "finish": "length" })), &[]);
    conn.execute("INSERT INTO todo VALUES('ses_a', 'second', 'in_progress', 'high', 1, 0, 0), ('ses_a', 'first', 'completed', 'low', 0, 0, 0)", []).unwrap();
    drop(conn);
    let store = store_with(&d.0, &["c:\\users\\kyle\\repo\\"]);

    let report = import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    assert_eq!((report.imported, report.known, report.failed.len()), (1, 0, 0), "{report:?}");
    let session = store.session("ses_a").unwrap().unwrap();
    assert_eq!(session.workspace_id, store.workspaces().unwrap()[0].id, "the same directory, spelled differently");
    assert_eq!((session.visibility, session.variant.as_deref(), session.model.unwrap().model.as_str()), (Visibility::Sibling, Some("high"), "claude-opus-5-5"));
    assert_eq!((session.created_at, session.updated_at, session.archived_at), (1000, 9000, None));

    let transcript = store.transcript("ses_a").unwrap();
    let ids: Vec<&str> = transcript.iter().map(|m| m.info.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "written order is id order");
    assert!(*ids.last().unwrap() < drift_engine::id::new("msg").as_str(), "a turn taken now sorts after the history");

    let prompt = &transcript[0];
    assert_eq!(prompt.parts[0].part, Part::Text { text: "fix it".into() });
    assert_eq!(prompt.parts[1].part, map::kept(&nudge.to_string()), "opencode's own nudge is kept, not shown or sent");
    assert_eq!(prompt.parts[2].part, Part::File { mime: "text/plain".into(), name: "a.rs".into(), url: "data:text/plain;base64,YQ==".into(), path: Some("src/a.rs".into()) });

    let reply = &transcript[1];
    assert_eq!((reply.info.role, reply.info.status, reply.info.cost), (Role::Assistant, MessageStatus::Done, 0.5));
    assert_eq!((reply.info.usage.input, reply.info.usage.output, reply.info.usage.cache_read, reply.info.usage.cache_write), (10, 25, 100, 7));
    let kinds: Vec<&Part> = reply.parts.iter().map(|row| &row.part).collect();
    assert_eq!(kinds.len(), 5, "step bookkeeping is dropped: {kinds:?}");
    assert_eq!(kinds[0], &Part::Reasoning { text: "think".into(), signature: Some("sig".into()), redacted: None }, "Claude's signature stays; replay sends it only to the model that wrote it");
    let Part::ToolCall { call_id, name, input, status, output, metadata, title, started_at, finished_at } = kinds[1] else { panic!() };
    assert_eq!((call_id.as_str(), name.as_str(), *status, output.as_deref(), title.as_deref()), ("toolu_1", "edit", ToolStatus::Done, Some("Edit applied successfully."), Some("a.rs")));
    assert_eq!((input["filePath"].as_str(), metadata.as_ref().unwrap()["diff"].as_str(), *started_at, *finished_at), (Some("a.rs"), Some("-a\n+b"), Some(2010), Some(2020)));
    assert!(matches!(kinds[2], Part::ToolCall { status: ToolStatus::Error, output: Some(output), .. } if output == "exit 1"));
    assert_eq!(kinds[3], &map::kept(&patch.to_string()));
    assert_eq!(kinds[4], &Part::Text { text: "done".into() });

    let Part::Compaction { auto: true, tail_from: Some(tail) } = &transcript[2].parts[0].part else { panic!("{:?}", transcript[2].parts) };
    assert_eq!(tail, &reply.info.id, "the boundary points at the new id of the message it kept");
    assert!(transcript[3].info.summary);
    assert_eq!((transcript[4].info.status, transcript[4].info.error.as_deref()), (MessageStatus::Aborted, Some("Aborted")));
    assert_eq!((transcript[5].info.status, transcript[5].info.ending), (MessageStatus::Done, Some(Ending::Length)));

    let todos: Vec<String> = store.todos("ses_a").unwrap().into_iter().map(|todo| todo.content).collect();
    assert_eq!(todos, ["first", "second"], "in opencode's order");
}

#[test]
fn subagents_follow_their_parent_and_a_rerun_brings_in_only_what_is_new() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_child", Some("ses_parent"), "D:/elsewhere", None);
    session(&conn, "ses_parent", None, "C:/repo", None);
    session(&conn, "ses_away", None, "E:/other", None);
    session(&conn, "ses_old", None, "C:/repo", Some(500));
    session(&conn, "ses_hidden", None, "C:/repo", None);
    session(&conn, "ses_sub", None, "C:/repo/crates/core", None);
    conn.execute_batch("INSERT INTO project VALUES('p_repo', 'C:/repo'); UPDATE session SET project_id = 'p_repo', time_updated = 9999 WHERE id = 'ses_sub';").unwrap();
    drop(conn);
    let store = store_with(&d.0, &["C:/repo"]);

    let before = drift_engine::id::now_ms();
    let mut announced = Vec::new();
    let first = import_sessions(&store, &source, &HashSet::from(["ses_hidden".to_string()]), &mut Kept::default(), &mut |step| if let Progress::Finished(Some(session)) = step { announced.push(session.id.clone()) }).unwrap();
    assert_eq!((first.imported, first.unmatched.get("E:/other")), (5, Some(&1)), "{first:?}");
    assert_eq!((announced.len(), announced.last().map(String::as_str)), (5, Some("ses_child")), "each announced as it lands, a subagent after its parent");
    assert_eq!(announced[0], "ses_sub", "the most recently used first");
    assert_eq!(store.session("ses_sub").unwrap().unwrap().workspace_id, store.workspaces().unwrap()[0].id, "run inside the repository the workspace holds");
    let child = store.session("ses_child").unwrap().unwrap();
    assert_eq!((child.visibility, child.parent_id.as_deref()), (Visibility::Hidden, Some("ses_parent")));
    assert_eq!(child.workspace_id, store.session("ses_parent").unwrap().unwrap().workspace_id, "a subagent lands with its parent");
    for archived in ["ses_old", "ses_hidden"] {
        assert!(store.session(archived).unwrap().unwrap().archived_at.unwrap() >= before, "{archived}: the week counts from the import");
    }

    store.lock().execute("DELETE FROM session WHERE id = 'ses_parent'", []).unwrap();
    store.add_workspace("e:\\other", "other", "").unwrap();
    let second = import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    assert_eq!((second.imported, second.known, second.unmatched.len()), (1, 5, 0), "{second:?}");
    assert!(store.session("ses_away").unwrap().is_some(), "a workspace added since brings its conversations in");
    assert!(store.session("ses_parent").unwrap().is_none(), "a deleted import stays deleted");
}

#[test]
fn a_conversation_a_stopped_run_left_half_written_is_finished_by_the_next_and_progress_counts_every_one() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    session(&conn, "ses_b", None, "C:/repo", None);
    message(&conn, "ses_a", "msg_1", 2000, user(2000), &[json!({ "type": "text", "text": "hello" })]);
    drop(conn);
    let store = store_with(&d.0, &["C:/repo"]);
    let workspace = store.workspaces().unwrap()[0].id.clone();
    let half = drift_engine::session::types::Session { id: "ses_a".into(), workspace_id: workspace, parent_id: None, visibility: Visibility::Sibling, title: "half".into(), agent: "build".into(), model: None, variant: None, created_at: 1, updated_at: 1, archived_at: None, branch_cutoff: None, revert: None, running: false };
    assert!(store.begin_import(&half).unwrap(), "a run that stopped after starting this one");

    let mut steps = Vec::new();
    let report = import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |step| steps.push(match step {
        Progress::Planned(total) => format!("planned {total}"),
        Progress::Finished(session) => format!("finished {}", session.is_some()),
    }))
    .unwrap();
    assert_eq!((report.imported, report.known), (2, 0), "{report:?}");
    assert_eq!(steps, ["planned 2", "finished true", "finished true"]);
    assert!(report.pending.is_empty(), "a database without opencode's queue has nothing pending");
    assert_eq!(store.transcript("ses_a").unwrap().len(), 1, "written whole this time");
    let again = import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| panic!("nothing left to report")).unwrap();
    assert_eq!(again.known, 2);
}

#[test]
fn a_conversation_with_prompts_opencode_queued_but_never_ran_is_named() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    session(&conn, "ses_b", None, "C:/repo", None);
    conn.execute_batch(
        "CREATE TABLE session_input(id TEXT PRIMARY KEY, session_id TEXT, prompt TEXT, delivery TEXT, admitted_seq INTEGER, promoted_seq INTEGER, time_created INTEGER);
         INSERT INTO session_input VALUES('i1', 'ses_a', '{}', 'queue', 1, NULL, 1), ('i2', 'ses_b', '{}', 'queue', 1, 2, 1);",
    )
    .unwrap();
    drop(conn);
    let store = store_with(&d.0, &["C:/repo"]);
    let report = import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    assert_eq!(report.pending, ["Title ses_a"], "a prompt that ran is in the transcript; one that never ran is named");
}

#[test]
fn display_copies_no_view_reads_are_left_behind_and_a_patched_files_diff_becomes_its_panel() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    let file = json!({ "filePath": "C:/repo/a.rs", "relativePath": "a.rs", "type": "update", "additions": 1, "deletions": 1, "before": "a\n", "after": "b\n", "diff": "@@ -1 +1 @@\n-a\n+b" });
    let patch = json!({ "type": "tool", "tool": "apply_patch", "callID": "c1", "state": { "status": "completed", "input": { "patchText": "*** Begin Patch" }, "output": "Success.", "metadata": { "diff": "whole", "files": [file] } } });
    let read = json!({ "type": "tool", "tool": "read", "callID": "c2", "state": { "status": "completed", "input": { "filePath": "a.rs" }, "output": "1: a", "metadata": { "display": "a", "preview": "a", "truncated": false } } });
    message(&conn, "ses_a", "msg_1", 2000, assistant(2000, json!({})), &[patch, read]);
    drop(conn);
    let store = store_with(&d.0, &["C:/repo"]);
    import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    let parts = store.transcript("ses_a").unwrap().remove(0).parts;
    let Part::ToolCall { metadata: Some(patched), .. } = &parts[0].part else { panic!() };
    assert_eq!(patched, &json!({ "diff": "whole", "files": [{ "filePath": "C:/repo/a.rs", "relativePath": "a.rs", "type": "update", "additions": 1, "deletions": 1, "patch": "@@ -1 +1 @@\n-a\n+b" }] }));
    let Part::ToolCall { metadata: Some(read), output, .. } = &parts[1].part else { panic!() };
    assert_eq!((read, output.as_deref()), (&json!({ "truncated": false }), Some("1: a")), "what the model read stays");
}

#[test]
fn a_diff_too_big_for_any_panel_is_dropped_and_the_call_keeps_its_output() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    let huge = "+x\n".repeat(400_000);
    let file = json!({ "filePath": "gen.rs", "additions": 400000, "deletions": 0, "diff": huge });
    let patch = json!({ "type": "tool", "tool": "apply_patch", "callID": "c1", "state": { "status": "completed", "input": {}, "output": "Success.", "metadata": { "diff": huge, "files": [file] } } });
    message(&conn, "ses_a", "msg_1", 2000, assistant(2000, json!({})), &[patch]);
    drop(conn);
    let store = store_with(&d.0, &["C:/repo"]);
    import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    let Part::ToolCall { metadata: Some(metadata), output, .. } = &store.transcript("ses_a").unwrap()[0].parts[0].part else { panic!() };
    assert_eq!((metadata, output.as_deref()), (&json!({ "files": [{ "filePath": "gen.rs", "additions": 400000, "deletions": 0 }] }), Some("Success.")));
}

#[test]
fn a_tool_call_stored_past_the_size_limit_keeps_its_name_input_and_output_and_nothing_else() {
    let d = dir();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    let huge = "x".repeat(9_000_000);
    let patch = json!({ "type": "tool", "tool": "apply_patch", "callID": "c1", "state": { "status": "completed", "input": { "patchText": "*** Begin Patch" }, "output": "Success.", "title": "gen.rs", "metadata": { "diff": huge, "files": [{ "filePath": "gen.rs", "before": huge }] }, "time": { "start": 5, "end": 6 } } });
    let image = json!({ "type": "file", "mime": "image/png", "filename": "big.png", "url": format!("data:image/png;base64,{huge}") });
    message(&conn, "ses_a", "msg_1", 2000, assistant(2000, json!({})), &[patch, image]);
    drop(conn);
    let store = store_with(&d.0, &["C:/repo"]);
    import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    let parts = store.transcript("ses_a").unwrap().remove(0).parts;
    let Part::ToolCall { call_id, name, input, status, output, title, metadata, started_at, finished_at } = &parts[0].part else { panic!("{:?}", parts[0].part) };
    assert_eq!((call_id.as_str(), name.as_str(), *status, output.as_deref(), title.as_deref()), ("c1", "apply_patch", ToolStatus::Done, Some("Success."), Some("gen.rs")));
    assert_eq!((input, metadata, *started_at, *finished_at), (&json!({ "patchText": "*** Begin Patch" }), &None, Some(5), Some(6)));
    assert!(matches!(&parts[1].part, Part::File { url, .. } if url.len() > 9_000_000), "anything else that large is read whole");
}

fn edit(path: &Path, diff: &str) -> serde_json::Value {
    json!({ "type": "tool", "tool": "edit", "callID": format!("e{}", path.display()), "state": { "status": "completed", "input": { "filePath": path, "oldString": "x", "newString": "y" }, "output": "Edit applied successfully.", "metadata": { "filediff": { "file": path, "patch": diff } } } })
}

/// Each recorded change of the conversation's calls, by path, with the content its versions hold.
fn recorded(store: &Store, session: &str, kept: &Kept) -> Vec<(String, Option<String>, Option<String>)> {
    let content = |blob: &serde_json::Value| blob.as_str().map(|id| kept.0[id].clone());
    let mut changes = Vec::new();
    for message in store.transcript(session).unwrap() {
        for row in message.parts {
            let Part::ToolCall { metadata: Some(metadata), .. } = row.part else { continue };
            assert!(metadata.get("changes").is_none() || metadata["at"] == message.info.id.as_str(), "stamped with its own message");
            for change in metadata["changes"].as_array().into_iter().flatten() {
                changes.push((change["path"].as_str().unwrap().to_string(), content(&change["before"]), content(&change["after"])));
            }
        }
    }
    changes.sort();
    changes
}

#[test]
fn recent_edits_are_rebuilt_from_todays_files_and_anything_that_no_longer_matches_is_left_out() {
    let d = dir();
    let ws = d.0.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let file = |name: &str, content: &str| std::fs::write(ws.join(name), content).unwrap();
    file("a.rs", "one\nTWO\nthree\n");
    file("new.rs", "fresh\n");
    file("moved.rs", "y\n");
    file("chain.rs", "c\n");
    file("touched.rs", "4\n");
    file("old.rs", "late\n");
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, &ws.to_string_lossy(), None);
    let now = drift_engine::id::now_ms();
    let hour = 3_600_000;
    let write_new = json!({ "type": "tool", "tool": "write", "callID": "w1", "state": { "status": "completed", "input": { "filePath": ws.join("new.rs"), "content": "fresh\n" }, "output": "Wrote.", "metadata": { "exists": false } } });
    let patch = json!({ "type": "tool", "tool": "apply_patch", "callID": "p1", "state": { "status": "completed", "input": {}, "output": "Success.", "metadata": { "files": [
        { "filePath": ws.join("gone.rs"), "type": "delete", "patch": "@@ -1 +0,0 @@\n-bye\n", "additions": 0, "deletions": 1 },
        { "filePath": ws.join("was.rs"), "movePath": ws.join("moved.rs"), "type": "move", "patch": "@@ -1 +1 @@\n-x\n+y\n", "additions": 1, "deletions": 1 },
    ] } } });
    message(&conn, "ses_a", "msg_0", now - 9 * 24 * hour, assistant(now - 9 * 24 * hour, json!({})), &[edit(&ws.join("old.rs"), "@@ -1 +1 @@\n-early\n+late\n")]);
    message(&conn, "ses_a", "msg_1", now - 3 * hour, assistant(now - 3 * hour, json!({})), &[edit(&ws.join("chain.rs"), "@@ -1 +1 @@\n-a\n+b\n"), edit(&ws.join("touched.rs"), "@@ -1 +1 @@\n-1\n+2\n")]);
    message(&conn, "ses_a", "msg_2", now - 2 * hour, assistant(now - 2 * hour, json!({})), &[edit(&ws.join("chain.rs"), "@@ -1 +1 @@\n-b\n+c\n"), edit(&ws.join("touched.rs"), "@@ -1 +1 @@\n-2\n+3\n")]);
    message(&conn, "ses_a", "msg_3", now - hour, assistant(now - hour, json!({})), &[edit(&ws.join("a.rs"), "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n"), write_new, patch]);
    drop(conn);
    let store = store_with(&d.0, &[&ws.to_string_lossy()]);
    let mut kept = Kept::default();
    let report = import_sessions(&store, &source, &HashSet::new(), &mut kept, &mut |_| {}).unwrap();
    assert_eq!(report.undoable, 5, "{report:?}");
    let some = |text: &str| Some(text.to_string());
    assert_eq!(recorded(&store, "ses_a", &kept), vec![
        ("a.rs".into(), some("one\ntwo\nthree\n"), some("one\nTWO\nthree\n")),
        ("chain.rs".into(), some("a\n"), some("b\n")),
        ("chain.rs".into(), some("b\n"), some("c\n")),
        ("gone.rs".into(), some("bye\n"), None),
        ("moved.rs".into(), None, some("y\n")),
        ("new.rs".into(), None, some("fresh\n")),
        ("was.rs".into(), some("x\n"), None),
    ], "touched.rs changed since, so neither of its edits is recorded; old.rs is over a week old");
}

#[test]
fn only_a_conversations_newest_thirty_messages_get_undo_records() {
    let d = dir();
    let ws = d.0.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.rs"), "b\n").unwrap();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, &ws.to_string_lossy(), None);
    let now = drift_engine::id::now_ms();
    message(&conn, "ses_a", "msg_000", now - 1000, assistant(now - 1000, json!({})), &[edit(&ws.join("a.rs"), "@@ -1 +1 @@\n-a\n+b\n")]);
    for n in 1..=30 {
        message(&conn, "ses_a", &format!("msg_{n:03}"), now - 1000 + n, user(now - 1000 + n), &[json!({ "type": "text", "text": "more" })]);
    }
    drop(conn);
    let store = store_with(&d.0, &[&ws.to_string_lossy()]);
    let report = import_sessions(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    assert_eq!((report.imported, report.undoable), (1, 0));
}

#[test]
fn an_imported_edit_undoes_and_redoes_through_the_engine_and_the_rest_are_named() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let d = dir();
    let ws = d.0.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.rs"), "after\n").unwrap();
    std::fs::write(ws.join("z.rs"), "someone else's\n").unwrap();
    let source = d.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, &ws.to_string_lossy(), None);
    let now = drift_engine::id::now_ms();
    message(&conn, "ses_a", "msg_1", now - 2000, user(now - 2000), &[json!({ "type": "text", "text": "fix" })]);
    message(&conn, "ses_a", "msg_2", now - 1000, assistant(now - 1000, json!({})), &[edit(&ws.join("a.rs"), "@@ -1 +1 @@\n-before\n+after\n"), edit(&ws.join("z.rs"), "@@ -1 +1 @@\n-z\n+zz\n")]);
    drop(conn);
    let engine = drift_engine::Engine::open_with(&d.0.join("data"), drift_engine::Options { file_credentials: true, ..Default::default() }).unwrap();
    engine.store.add_workspace(&ws.to_string_lossy(), "ws", "").unwrap();
    let mut history = History::new(&engine.snapshots).unwrap();
    let report = import_sessions(&engine.store, &source, &HashSet::new(), &mut history, &mut |_| {}).unwrap();
    drop(history);
    assert_eq!((report.imported, report.undoable), (1, 1), "{report:?}");
    let prompt = engine.store.transcript("ses_a").unwrap()[0].info.id.clone();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let undone = runtime.block_on(engine.revert("ses_a", &prompt)).unwrap();
    assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), "before\n");
    assert_eq!(std::fs::read_to_string(ws.join("z.rs")).unwrap(), "someone else's\n", "a file whose diff no longer matches is left alone");
    assert_eq!(undone.unrecorded, [ws.join("z.rs").to_string_lossy()], "and named");
    runtime.block_on(engine.unrevert("ses_a")).unwrap();
    assert_eq!(std::fs::read_to_string(ws.join("a.rs")).unwrap(), "after\n", "redo puts the edit back");
}

#[test]
fn directories_compare_without_case_slash_style_or_a_trailing_slash() {
    assert_eq!(directory_key("C:\\Users\\Kyle\\Repo\\"), directory_key("c:/users/kyle/repo"));
    assert_eq!(directory_key("C:\\"), "c:/");
    assert_eq!(directory_key("C:/"), "c:/");
    assert_eq!(directory_key("/"), "/");
    assert_ne!(directory_key("C:/repo"), directory_key("C:/repo2"));
}
