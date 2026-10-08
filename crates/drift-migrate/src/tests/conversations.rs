use super::*;
use drift_engine::session::types::{MessageWithParts, Session};

fn write_prompt(conversation: &Conversation<'_>, nudge: &Value) {
    conversation.message(
        "msg_1",
        2000,
        user(2000),
        &[
            json!({ "type": "text", "text": "fix it" }),
            nudge.clone(),
            json!({
                "type": "file", "mime": "text/plain", "filename": "a.rs", "url": "data:text/plain;base64,YQ==",
                "source": { "type": "file", "path": "src/a.rs" },
            }),
        ],
    );
}

fn write_reply(conversation: &Conversation<'_>, patch: &Value) {
    let edit = json!({
        "type": "tool", "tool": "edit", "callID": "toolu_1",
        "state": {
            "status": "completed", "input": { "filePath": "a.rs", "oldString": "a", "newString": "b" },
            "output": "Edit applied successfully.", "title": "a.rs", "metadata": { "diff": "-a\n+b" },
            "time": { "start": 2010, "end": 2020 },
        },
    });
    let failed = json!({
        "type": "tool", "tool": "bash", "callID": "toolu_2",
        "state": {
            "status": "error", "input": { "command": "boom" }, "error": "exit 1",
            "time": { "start": 2021, "end": 2022 },
        },
    });

    conversation.message(
        "msg_2",
        2005,
        assistant(2005, json!({})),
        &[
            json!({ "type": "step-start", "snapshot": "x" }),
            json!({ "type": "reasoning", "text": "think", "metadata": { "anthropic": { "signature": "sig" } } }),
            edit,
            failed,
            patch.clone(),
            json!({ "type": "step-finish", "reason": "stop" }),
            json!({ "type": "text", "text": "done" }),
        ],
    );
}

fn write_later_messages(conversation: &Conversation<'_>) {
    conversation.message(
        "msg_3",
        3000,
        user(3000),
        &[json!({ "type": "compaction", "auto": true, "tail_start_id": "msg_2" })],
    );
    conversation.message(
        "msg_4",
        3001,
        assistant(3001, json!({ "summary": true })),
        &[json!({ "type": "text", "text": "summary" })],
    );
    conversation.message(
        "msg_5",
        4000,
        assistant(
            4000,
            json!({
                "error": { "name": "MessageAbortedError", "data": { "message": "Aborted" } },
            }),
        ),
        &[],
    );
    conversation.message("msg_6", 5000, assistant(5000, json!({ "finish": "length" })), &[]);
}

fn assert_session(session: Session, store: &Store) {
    assert_eq!(
        session.workspace_id,
        store.workspaces().unwrap()[0].id,
        "the same directory, spelled differently"
    );
    assert_eq!(
        (
            session.visibility,
            session.variant.as_deref(),
            session.model.unwrap().model.as_str()
        ),
        (Visibility::Sibling, Some("high"), "claude-opus-5-5")
    );
    assert_eq!(
        (session.created_at, session.updated_at, session.archived_at),
        (1000, 9000, None)
    );
}

fn assert_prompt(prompt: &MessageWithParts, nudge: &Value) {
    assert_eq!(prompt.parts[0].part, Part::Text { text: "fix it".into() });
    assert_eq!(
        prompt.parts[1].part,
        map::kept(&nudge.to_string()),
        "opencode's own nudge is kept, not shown or sent"
    );
    assert_eq!(
        prompt.parts[2].part,
        Part::File {
            mime: "text/plain".into(),
            name: "a.rs".into(),
            url: "data:text/plain;base64,YQ==".into(),
            path: Some("src/a.rs".into()),
        }
    );
}

fn assert_edit(part: &Part) {
    let Part::ToolCall {
        call_id,
        name,
        input,
        status,
        output,
        metadata,
        title,
        started_at,
        finished_at,
    } = part
    else {
        panic!("expected an edit call")
    };

    assert_eq!(
        (
            call_id.as_str(),
            name.as_str(),
            *status,
            output.as_deref(),
            title.as_deref()
        ),
        (
            "toolu_1",
            "edit",
            ToolStatus::Done,
            Some("Edit applied successfully."),
            Some("a.rs"),
        )
    );
    assert_eq!(
        (
            input["filePath"].as_str(),
            metadata.as_ref().unwrap().diff.as_deref(),
            *started_at,
            *finished_at
        ),
        (Some("a.rs"), Some("-a\n+b"), Some(2010), Some(2020))
    );
}

fn assert_reply(reply: &MessageWithParts, patch: &Value) {
    assert_eq!(
        (reply.info.role, reply.info.status, reply.info.cost),
        (Role::Assistant, MessageStatus::Done, 0.5)
    );
    assert_eq!(
        (
            reply.info.usage.input,
            reply.info.usage.output,
            reply.info.usage.cache_read,
            reply.info.usage.cache_write
        ),
        (10, 25, 100, 7)
    );

    let kinds: Vec<&Part> = reply.parts.iter().map(|row| &row.part).collect();
    assert_eq!(kinds.len(), 5, "step bookkeeping is dropped: {kinds:?}");
    assert_eq!(
        kinds[0],
        &Part::Reasoning {
            text: "think".into(),
            signature: Some("sig".into()),
            redacted: None,
        },
        "Claude's signature stays; replay sends it only to the model that wrote it"
    );
    assert_edit(kinds[1]);
    assert!(
        matches!(kinds[2], Part::ToolCall { status: ToolStatus::Error, output: Some(output), .. } if output == "exit 1")
    );
    assert_eq!(kinds[3], &map::kept(&patch.to_string()));
    assert_eq!(kinds[4], &Part::Text { text: "done".into() });
}

fn assert_later_messages(transcript: &[MessageWithParts]) {
    let Part::Compaction {
        auto: true,
        tail_from: Some(tail),
    } = &transcript[2].parts[0].part
    else {
        panic!("{:?}", transcript[2].parts);
    };

    assert_eq!(
        tail, &transcript[1].info.id,
        "the boundary points at the new id of the message it kept"
    );
    assert!(transcript[3].info.summary);
    assert_eq!(
        (transcript[4].info.status, transcript[4].info.error.as_deref()),
        (MessageStatus::Aborted, Some("Aborted"))
    );
    assert_eq!(
        (transcript[5].info.status, transcript[5].info.ending),
        (MessageStatus::Done, Some(Ending::Length))
    );
}

#[test]
fn a_conversation_arrives_in_its_workspace_with_every_part_in_this_engines_shape() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/Users/Kyle/Repo", None);
    let conversation = Conversation::new(&conn, "ses_a");
    let patch = json!({ "type": "patch", "hash": "abc", "files": ["a.rs"] });
    let nudge = json!({ "type": "text", "synthetic": true, "text": "Continue." });

    write_prompt(&conversation, &nudge);
    write_reply(&conversation, &patch);
    write_later_messages(&conversation);
    conn.execute(
        "INSERT INTO todo VALUES('ses_a', 'second', 'in_progress', 'high', 1, 0, 0),
                                ('ses_a', 'first', 'completed', 'low', 0, 0, 0)",
        [],
    )
    .unwrap();
    drop(conn);
    let store = store_with(&directory.0, &["c:\\users\\kyle\\repo\\"]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!(
        (report.imported, report.known, report.failed.len()),
        (1, 0, 0),
        "{report:?}"
    );
    assert_session(store.session("ses_a").unwrap().unwrap(), &store);

    let transcript = store.transcript("ses_a").unwrap();
    let ids: Vec<&str> = transcript.iter().map(|message| message.info.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();

    assert_eq!(ids, sorted, "written order is id order");
    assert!(
        *ids.last().unwrap() < drift_engine::id::new("msg").as_str(),
        "a turn taken now sorts after the history"
    );
    assert_prompt(&transcript[0], &nudge);
    assert_reply(&transcript[1], &patch);
    assert_later_messages(&transcript);

    let todos: Vec<String> = store
        .todos("ses_a")
        .unwrap()
        .into_iter()
        .map(|todo| todo.content)
        .collect();
    assert_eq!(todos, ["first", "second"], "in opencode's order");
}
