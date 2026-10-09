use super::*;

const METADATA_SAMPLES: &[(&str, &str)] = &[
    (
        "bash",
        concat!(
            r#"{"shellTimeoutMs": 120000, "outputBytes": 42, "outputFile": "C:/work/command.log", "#,
            r#""exit": 3, "notes": ["exit code 3"]}"#,
        ),
    ),
    (
        "unlimited bash",
        r#"{"shellTimeoutMs": null, "outputBytes": 0, "stopped": true}"#,
    ),
    (
        "running bash",
        r#"{"shellTimeoutMs": 400, "output": "Building application", "timedOut": true}"#,
    ),
    (
        "edit",
        r#"{
        "replacements": 1, "files": ["C:/work/main.rs"], "diff": "-old\n+new",
        "fileChanges": [{"filePath": "C:/work/main.rs", "relativePath": "main.rs",
            "type": "update", "patch": "-old\n+new", "additions": 1, "deletions": 1}]
    }"#,
    ),
    (
        "write",
        r#"{
        "created": false, "files": ["C:/work/main.rs"], "diff": "-old\n+new",
        "fileChanges": [{"filePath": "C:/work/main.rs", "relativePath": "main.rs",
            "type": "add", "patch": "+new", "additions": 1, "deletions": 0}]
    }"#,
    ),
    (
        "apply_patch",
        r#"{
        "files": ["C:/work/moved.rs"], "fileChanges": [
            {"filePath": "C:/work/moved.rs", "relativePath": "moved.rs", "type": "move",
                "patch": "-old\n+new", "additions": 1, "deletions": 1},
            {"filePath": "C:/work/removed.rs", "relativePath": "removed.rs", "type": "delete",
                "patch": "-old", "additions": 0, "deletions": 1}]
    }"#,
    ),
    ("read", r#"{"lines": 20, "shown": 10}"#),
    ("large read", r#"{"lines": null, "shown": 100, "large": true}"#),
    (
        "grep",
        r#"{"count": 200, "total": 2000, "capped": true, "truncated": true, "withheld": 1, "restricted": 2}"#,
    ),
    ("glob", r#"{"count": 1, "total": 1, "truncated": false}"#),
    ("webfetch", r#"{"contentType": "text/html", "bytes": 256}"#),
    ("webfetch redirect", r#"{"redirect": "https://example.org/document"}"#),
    (
        "task",
        r#"{
        "sessionId": "ses_worker", "taskId": "task_review", "agent": "explore", "outcome": "replied",
        "mode": "foreground", "delivers": "task_review"
    }"#,
    ),
    (
        "background task",
        r#"{
        "sessionId": "ses_worker", "taskId": "task_review", "agent": "explore", "outcome": "launched",
        "mode": "background", "reason": "explicit"
    }"#,
    ),
    (
        "task_output",
        r#"{"sessionId": "ses_worker", "taskId": "task_review", "state": "running"}"#,
    ),
    ("read_thread", r#"{"sessionId": "ses_worker", "running": false}"#),
    ("question", r#"{"requestId": "qst_database", "async": true}"#),
    (
        "question answers",
        r#"{"answers": [["SQLite"], ["Keep existing tables"]]}"#,
    ),
    ("mcp", r#"{"server": "documents", "uri": "file:///guide.pdf"}"#),
    (
        "checks/history",
        r#"{
        "changes": [{"path": "main.rs", "before": null, "after": "blob_new"},
            {"path": "notes.txt", "before": "blob_old", "after": null, "observed": true}],
        "owner": "ws_project", "at": "chg_written", "unrecorded": ["large.log"],
        "checks": [{"check": "lint", "status": "passed"},
            {"check": "types", "status": "problems", "output": "Missing type annotation"},
            {"check": "build", "status": "unavailable", "output": "Compiler unavailable"}],
        "checkChanged": ["main.rs"], "checkObserved": ["notes.txt"], "formatted": ["rustfmt: main.rs"],
        "diagnostics": [{"file": "main.rs", "server": "rust-analyzer", "line": 3, "column": 1,
            "message": "Missing type annotation"}], "notes": ["A formatter changed main.rs"]
    }"#,
    ),
    (
        "history failure",
        r#"{"changes": [], "owner": "ws_project", "unrecorded": [], "historyError": "History unavailable"}"#,
    ),
    (
        "returned images",
        r#"{"images": [{"mime": "image/png", "data": "iVBORw=="}]}"#,
    ),
    (
        "stored images",
        r#"{"images": [{"mime": "application/pdf", "hash": "abc123"}]}"#,
    ),
    (
        "skill",
        r#"{"path": "C:/skills/review", "files": ["C:/skills/review/guide.md"]}"#,
    ),
    ("todowrite", r#"{"count": 3, "open": 1}"#),
    (
        "command",
        r#"{"engineCommand": "review", "commandModel": "anthropic/claude"}"#,
    ),
    ("spilled result", r#"{"resultFile": "C:/work/result.log"}"#),
];

#[test]
fn tool_metadata_producers_round_trip_without_added_defaults() {
    for (producer, sample) in METADATA_SAMPLES {
        let json: Value = serde_json::from_str(sample).unwrap();
        let metadata: ToolMetadata = serde_json::from_value(json.clone()).unwrap();
        assert!(metadata.extra.is_empty(), "{producer}: native fields must be typed");
        assert_eq!(serde_json::to_value(&metadata).unwrap(), json, "{producer}");
    }

    assert_eq!(
        serde_json::to_value(ToolMetadata::default()).unwrap(),
        serde_json::json!({})
    );
}

#[test]
fn tool_metadata_samples_round_trip_in_stored_parts() {
    for (producer, sample) in METADATA_SAMPLES {
        let metadata: Value = serde_json::from_str(sample).unwrap();
        let stored = serde_json::json!({
            "type": "tool_call",
            "callId": "call_saved",
            "name": producer,
            "input": {},
            "status": "done",
            "title": "Saved result",
            "output": "Complete",
            "metadata": metadata,
            "startedAt": 1000,
            "finishedAt": 2000,
        });

        let part = Part::from_stored(&stored.to_string());
        assert!(matches!(part, Part::ToolCall { metadata: Some(_), .. }), "{producer}");
        assert_eq!(
            serde_json::from_str::<Value>(&part.stored()).unwrap(),
            stored,
            "{producer}"
        );
    }
}

#[test]
fn tool_metadata_schema_declares_the_producers_wire_keys() {
    use utoipa::PartialSchema;

    let schema = serde_json::to_value(ToolMetadata::schema()).unwrap();
    let properties = schema["properties"].as_object().unwrap();

    for (producer, sample) in METADATA_SAMPLES {
        let metadata: Value = serde_json::from_str(sample).unwrap();
        for key in metadata.as_object().unwrap().keys() {
            assert!(properties.contains_key(key), "{producer}: schema lacks {key}");
        }
    }

    assert!(!properties.contains_key("legacy"));
    assert!(!properties.contains_key("extra"));
    assert!(!properties.contains_key("engine_command"));
    assert!(schema.get("additionalProperties").is_some());
}

#[test]
fn typed_metadata_overwrites_legacy_keys_without_duplicate_json_fields() {
    let mut metadata = ToolMetadata::from(serde_json::json!({"exit": "unavailable", "notes": null}));
    metadata.exit = Some(0);
    metadata.notes = Some(Vec::new());

    let json = serde_json::to_string(&metadata).unwrap();
    assert_eq!(json.matches("\"exit\"").count(), 1);
    assert_eq!(json.matches("\"notes\"").count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap(),
        serde_json::json!({"exit": 0, "notes": []})
    );

    let output = crate::tool::Output::new("Read directory", "No files");
    assert!(serde_json::to_value(output).unwrap().get("metadata").is_none());
}

#[test]
fn imported_metadata_and_non_objects_stay_loadable() {
    let samples = [
        serde_json::json!({
            "filediff": { "file": "C:/work/main.rs", "patch": "-old\n+new" },
            "files": [{ "filePath": "C:/work/main.rs", "movePath": "C:/work/new.rs", "custom": 7 }],
            "changes": [{ "path": "main.rs" }],
            "at": "msg_imported",
            "future": { "version": 2 },
        }),
        serde_json::json!({"notes": null, "exit": "unknown", "images": [{"mime": "image/png", "data": null}]}),
        serde_json::json!({"checks": [{"check": "lint", "status": "passed", "output": null}]}),
        serde_json::json!({"changes": [{"path": "main.rs", "observed": null}]}),
        serde_json::json!(["old metadata", 7]),
        serde_json::json!("old metadata"),
        serde_json::json!(42),
        serde_json::json!(false),
    ];

    for json in samples {
        let metadata: ToolMetadata = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(metadata).unwrap(), json);

        let stored = serde_json::json!({
            "type": "tool_call",
            "callId": "call_imported",
            "name": "read",
            "input": {},
            "status": "done",
            "metadata": json,
        });

        let part = Part::from_stored(&stored.to_string());
        assert!(matches!(part, Part::ToolCall { .. }));
        assert_eq!(serde_json::from_str::<Value>(&part.stored()).unwrap(), stored);
    }

    let null: ToolMetadata = serde_json::from_value(Value::Null).unwrap();
    assert!(null.is_null());
    assert_eq!(serde_json::to_value(null).unwrap(), Value::Null);
}

#[test]
fn typed_metadata_merges_with_the_same_json_precedence() {
    let base = ToolMetadata::from(serde_json::json!({"exit": "unknown", "notes": ["Earlier"], "future": 1}));
    let patch = ToolMetadata::from(serde_json::json!({"exit": 0, "notes": [], "future": 2}));
    let merged = base.merged(Some(patch)).unwrap();

    assert_eq!(
        serde_json::to_value(&merged).unwrap(),
        serde_json::json!({"exit": 0, "notes": [], "future": 2})
    );

    let merged = merged
        .merged(Some(serde_json::json!({"exit": "unavailable"}).into()))
        .unwrap();
    assert!(merged.exit.is_none());
    assert_eq!(serde_json::to_value(merged).unwrap()["exit"], "unavailable");

    assert!(ToolMetadata::null().merged(None).is_none());

    let legacy = ToolMetadata::from(serde_json::json!([1]));
    assert_eq!(legacy.clone().merged(Some(ToolMetadata::default())), Some(legacy));
}

#[test]
fn part_serialises_tagged_and_flat_in_row() {
    let row = PartRow {
        id: "prt_1".into(),
        message_id: "msg_1".into(),
        session_id: "ses_1".into(),
        provider_signature: None,
        part: Part::ToolCall {
            call_id: "toolu_1".into(),
            name: "read".into(),
            input: serde_json::json!({ "path": "a.rs" }),
            status: ToolStatus::Pending,
            title: None,
            output: None,
            metadata: None,
            started_at: None,
            finished_at: None,
        },
    };

    let json = serde_json::to_value(&row).unwrap();
    assert_eq!(json["type"], "tool_call");
    assert_eq!(json["callId"], "toolu_1");
    assert_eq!(json["messageId"], "msg_1");
    assert!(json.get("output").is_none());

    let back: PartRow = serde_json::from_value(json).unwrap();
    assert_eq!(back, row);
}

#[test]
fn a_part_never_shadows_its_rows_own_fields() {
    let row = PartRow {
        id: "prt_1".into(),
        message_id: "msg_1".into(),
        session_id: "ses_parent".into(),
        provider_signature: None,
        part: Part::TaskResult {
            task_id: "task_1".into(),
            worker_session_id: "ses_worker".into(),
            description: "d".into(),
            outcome: "replied".into(),
            text: "t".into(),
        },
    };

    let text = serde_json::to_string(&row).unwrap();
    assert_eq!(text.matches("\"sessionId\"").count(), 1, "{text}");

    let back: PartRow = serde_json::from_str(&text).unwrap();
    assert_eq!(back, row);
}
