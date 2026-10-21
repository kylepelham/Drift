use super::*;
use drift_engine::session::types::MessageWithParts;

const V2_TABLES: &str = "CREATE TABLE session_v2(
        id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT, directory TEXT NOT NULL, title TEXT,
        agent TEXT, model TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER
    );
    CREATE TABLE session_message(
        id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL,
        time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
    );
    CREATE TABLE session_inbox(
        id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, payload TEXT NOT NULL,
        delivery TEXT NOT NULL, enqueued_seq INTEGER NOT NULL, time_created INTEGER NOT NULL
    );";

/// opencode.db as v2 leaves it: v1's tables kept, v2's beside them.
fn opencode_v2(path: &Path) -> Connection {
    let conn = opencode(path);
    conn.execute_batch(V2_TABLES).unwrap();

    conn
}

fn v2_session(conn: &Connection, id: &str, title: Option<&str>, updated: i64) {
    let model = json!({ "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" }).to_string();
    conn.execute(
        "INSERT INTO session_v2 VALUES(?1, 'p', NULL, 'C:/repo', ?2, 'build', ?3, 1000, ?4, NULL)",
        params![id, title, model, updated],
    )
    .unwrap();
}

/// Writes rows as v2 does: the type in its own column, the rest in `data`.
fn rows(conn: &Connection, session: &str, rows: &[Value]) {
    for (seq, row) in rows.iter().enumerate() {
        let mut data = row.clone();
        let kind = data.as_object_mut().unwrap().remove("type").unwrap();
        let created = data["time"]["created"].as_i64().unwrap();
        conn.execute(
            "INSERT INTO session_message VALUES(?1, ?2, ?3, ?4, ?5, ?5, ?6)",
            params![
                format!("msg_{session}_{seq:03}"),
                session,
                kind.as_str(),
                seq as i64,
                created,
                data.to_string()
            ],
        )
        .unwrap();
    }
}

fn reply() -> Value {
    json!({
        "type": "assistant", "agent": "build", "model": { "providerID": "anthropic", "id": "claude-opus-5-5" },
        "content": [
            { "type": "reasoning", "text": "think", "state": { "anthropic": { "signature": "sig" } } },
            {
                "type": "tool", "id": "toolu_1", "name": "edit",
                "state": {
                    "status": "completed", "input": { "path": "a.rs" }, "metadata": { "diff": "-a\n+b" },
                    "content": [
                        { "type": "text", "text": "Edited" },
                        { "type": "file", "uri": "data:image/png;base64,AA==", "mime": "image/png", "name": "shot.png" },
                    ],
                },
                "time": { "created": 2010, "completed": 2020 },
            },
            {
                "type": "tool", "id": "toolu_2", "name": "shell",
                "state": {
                    "status": "error", "input": { "command": "boom" },
                    "error": { "type": "tool.execution", "message": "exit 1" },
                },
                "time": { "created": 2021 },
            },
            { "type": "tool", "id": "toolu_3", "name": "read", "state": { "status": "streaming", "input": "{\"pa" }, "time": { "created": 2022 } },
            { "type": "citation", "url": "https://example.com" },
            { "type": "text", "text": "done" },
        ],
        "cost": 0.5, "tokens": { "input": 10, "output": 20, "reasoning": 5, "cache": { "read": 100, "write": 7 } },
        "time": { "created": 2005, "completed": 2030 },
    })
}

fn conversation() -> Vec<Value> {
    vec![
        json!({
            "type": "user", "text": "fix it", "time": { "created": 2000 }, "agents": [{ "name": "plan" }],
            "files": [{ "data": "YQ==", "mime": "text/plain", "name": "a.rs", "source": { "type": "inline" } }],
        }),
        reply(),
        json!({ "type": "idle", "outcome": "succeeded", "time": { "created": 2031 } }),
        json!({ "type": "system", "text": "AGENTS.md changed", "time": { "created": 2040 } }),
        json!({
            "type": "compaction", "status": "completed", "reason": "auto", "summary": "the summary",
            "recent": "[User]: fix it", "time": { "created": 3000 },
        }),
        json!({
            "type": "compaction", "status": "failed", "reason": "manual",
            "error": { "type": "provider.error", "message": "no" }, "time": { "created": 3500 },
        }),
        json!({
            "type": "assistant", "agent": "build", "model": { "providerID": "openai", "id": "gpt-6" }, "content": [],
            "error": { "type": "aborted", "message": "Aborted" }, "time": { "created": 4000 },
        }),
        json!({
            "type": "assistant", "agent": "build", "model": { "providerID": "openai", "id": "gpt-6" }, "content": [],
            "finish": "length", "time": { "created": 5000, "completed": 5001 },
        }),
        json!({ "type": "model-switched", "model": { "providerID": "openai", "id": "gpt-6" }, "time": { "created": 5002 } }),
    ]
}

/// The row a message holds raw, read back out of its envelope.
fn raw(message: &MessageWithParts) -> Value {
    let Part::Unknown { raw } = &message.parts[0].part else {
        panic!("{:?}", message.parts);
    };
    let envelope: Value = serde_json::from_str(raw).unwrap();

    serde_json::from_str(envelope["data"].as_str().unwrap()).unwrap()
}

fn assert_prompt(prompt: &MessageWithParts) {
    let parts: Vec<&Part> = prompt.parts.iter().map(|row| &row.part).collect();

    assert_eq!((prompt.info.role, prompt.info.created_at), (Role::User, 2000));
    assert_eq!(parts[0], &Part::Text { text: "fix it".into() });
    assert_eq!(
        parts[1],
        &Part::File {
            mime: "text/plain".into(),
            name: "a.rs".into(),
            url: "data:text/plain;base64,YQ==".into(),
            path: None,
            image: None,
        }
    );
    assert_eq!(
        parts[2],
        &map::kept(r#"{"agents":[{"name":"plan"}]}"#),
        "a mentioned agent is kept raw"
    );
}

fn assert_reply(message: &MessageWithParts) {
    assert_eq!(
        (
            message.info.status,
            message.info.cost,
            message.info.usage.output,
            message.info.finished_at
        ),
        (MessageStatus::Done, 0.5, 25, Some(2030))
    );
    let parts: Vec<&Part> = message.parts.iter().map(|row| &row.part).collect();
    assert_eq!(parts.len(), 6, "{parts:?}");
    assert!(matches!(parts[0], Part::Reasoning { signature: Some(signature), .. } if signature == "sig"));
    assert_edit(parts[1]);
    assert!(
        matches!(parts[2], Part::ToolCall { name, status: ToolStatus::Error, output: Some(output), .. }
            if name == "shell" && output == "exit 1"),
        "v2's names are kept"
    );
    assert!(
        matches!(parts[3], Part::ToolCall { status: ToolStatus::Error, output: None, input, .. } if input == &json!({}))
    );
    assert_eq!(parts[4], &map::kept(&reply()["content"][4].to_string()));
    assert_eq!(parts[5], &Part::Text { text: "done".into() });
}

fn assert_edit(part: &Part) {
    let Part::ToolCall {
        name,
        input,
        status,
        output,
        metadata,
        started_at,
        finished_at,
        ..
    } = part
    else {
        panic!("{part:?}");
    };
    assert_eq!(
        (name.as_str(), input["path"].as_str(), *status, output.as_deref()),
        ("edit", Some("a.rs"), ToolStatus::Done, Some("Edited\n[file shot.png]"))
    );
    assert_eq!(
        (metadata.as_ref().unwrap().diff.as_deref(), *started_at, *finished_at),
        (Some("-a\n+b"), Some(2010), Some(2020))
    );
}

fn assert_later(transcript: &[MessageWithParts], conversation: &[Value]) {
    assert_eq!(raw(&transcript[2]), conversation[3], "a system row is kept raw");
    assert_eq!(
        transcript[3].parts[0].part,
        Part::Compaction {
            auto: true,
            tail_from: None
        }
    );
    let summary: Vec<&Part> = transcript[4].parts.iter().map(|row| &row.part).collect();
    assert!(transcript[4].info.summary);
    assert_eq!(
        summary,
        [
            &Part::Text {
                text: "the summary".into()
            },
            &Part::Text {
                text: "[User]: fix it".into()
            }
        ],
        "v2's recent history rides with its summary"
    );
    assert_eq!(
        raw(&transcript[5]),
        conversation[5],
        "a failed compaction is no boundary"
    );
    assert_eq!(
        (transcript[6].info.status, transcript[6].info.error.as_deref()),
        (MessageStatus::Aborted, Some("Aborted"))
    );
    assert_eq!(
        (transcript[7].info.status, transcript[7].info.ending),
        (MessageStatus::Done, Some(Ending::Length))
    );
}

#[test]
fn a_v2_conversation_arrives_with_every_row_in_this_engines_shape() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode_v2(&source);
    v2_session(&conn, "ses_a", Some("Made in v2"), 9000);
    let conversation = conversation();
    rows(&conn, "ses_a", &conversation);
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!((report.imported, report.failed.len()), (1, 0), "{report:?}");
    let session = store.session("ses_a").unwrap().unwrap();
    assert_eq!(
        (session.title.as_str(), session.variant.as_deref()),
        ("Made in v2", Some("high"))
    );
    let transcript = store.transcript("ses_a").unwrap();
    assert_eq!(transcript.len(), 8, "bookkeeping rows are dropped");
    assert!(transcript.windows(2).all(|pair| pair[0].info.id < pair[1].info.id));
    assert_prompt(&transcript[0]);
    assert_reply(&transcript[1]);
    assert_later(&transcript, &conversation);
}

fn text(message: &MessageWithParts) -> &str {
    match &message.parts[0].part {
        Part::Text { text } => text,
        other => panic!("{other:?}"),
    }
}

fn user_row(text: &str, created: i64) -> Value {
    json!({ "type": "user", "text": text, "time": { "created": created } })
}

fn write_both_shapes(conn: &Connection) {
    for id in ["ses_old", "ses_went_on", "ses_next"] {
        session(conn, id, None, "C:/repo", None);
    }
    for id in ["ses_old", "ses_went_on"] {
        Conversation::new(conn, id).message(
            &format!("msg_{id}"),
            2000,
            user(2000),
            &[json!({ "type": "text", "text": "from v1" })],
        );
    }
    v2_session(conn, "ses_old", Some("Title ses_old"), 9000);
    rows(conn, "ses_old", &[user_row("copied by v2", 2000)]);
    v2_session(conn, "ses_went_on", Some("Renamed in v2"), 9500);
    rows(
        conn,
        "ses_went_on",
        &[user_row("copied by v2", 2000), user_row("carried on in v2", 6000)],
    );
    rows(conn, "ses_next", &[user_row("only in session_message", 2000)]);
}

#[test]
fn a_conversation_in_both_shapes_comes_from_v1_unless_it_went_on_in_v2() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode_v2(&source);
    write_both_shapes(&conn);
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!((report.imported, report.failed.len()), (3, 0), "each once: {report:?}");
    let old = store.transcript("ses_old").unwrap();
    assert_eq!(
        (old.len(), text(&old[0])),
        (1, "from v1"),
        "v1 keeps more than v2's copy"
    );
    let went_on = store.transcript("ses_went_on").unwrap();
    assert_eq!(
        went_on.iter().map(text).collect::<Vec<_>>(),
        ["copied by v2", "carried on in v2"]
    );
    assert_eq!(store.session("ses_went_on").unwrap().unwrap().title, "Renamed in v2");
    let next = store.transcript("ses_next").unwrap();
    assert_eq!(
        text(&next[0]),
        "only in session_message",
        "a session row whose messages are all v2's"
    );
}

#[test]
fn a_database_without_v1_sessions_still_imports_and_names_prompts_left_in_the_inbox() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = Connection::open(&source).unwrap();
    conn.execute_batch("CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT NOT NULL);")
        .unwrap();
    conn.execute_batch(V2_TABLES).unwrap();
    v2_session(&conn, "ses_queued", Some("Queued"), 9000);
    v2_session(&conn, "ses_untitled", None, 8000);
    rows(&conn, "ses_queued", &[user_row("hello", 2000)]);
    conn.execute(
        "INSERT INTO session_inbox VALUES('i1', 'ses_queued', 'user', '{}', 'queue', 1, 1)",
        [],
    )
    .unwrap();
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!(
        (report.imported, report.failed.len(), report.undoable),
        (2, 0, 0),
        "{report:?}"
    );
    assert_eq!(report.pending, ["Queued"]);
    assert_eq!(store.session("ses_untitled").unwrap().unwrap().title, "");
}

#[test]
fn a_v2_row_past_the_size_limit_keeps_each_calls_name_and_output_prefix() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode_v2(&source);
    v2_session(&conn, "ses_a", Some("Big"), 9000);
    let huge = "x".repeat(9_000_000);
    let call = json!({
        "type": "tool", "id": "toolu_1", "name": "shell",
        "state": {
            "status": "completed", "input": { "command": "cat big" }, "metadata": { "output": huge },
            "content": [{ "type": "text", "text": huge }],
        },
        "time": { "created": 2010, "completed": 2020 },
    });
    let mut big = reply();
    big["content"] = json!([call]);
    rows(&conn, "ses_a", &[big]);
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    let parts = store.transcript("ses_a").unwrap().remove(0).parts;
    let Part::ToolCall {
        input,
        output,
        metadata,
        ..
    } = &parts[0].part
    else {
        panic!("{:?}", parts[0].part);
    };
    assert_eq!(input["command"], "cat big");
    assert_eq!(output.as_ref().map(String::len), Some(source::KEPT_BYTES as usize));
    assert!(metadata.is_none());
}
