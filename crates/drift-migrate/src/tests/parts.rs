use super::*;
use drift_engine::session::types::PartRow;

fn import_parts(parts: &[Value]) -> Vec<PartRow> {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    Conversation::new(&conn, "ses_a").message("msg_1", 2000, assistant(2000, json!({})), parts);
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    store.transcript("ses_a").unwrap().remove(0).parts
}

#[test]
fn display_copies_no_view_reads_are_left_behind_and_a_patched_files_diff_becomes_its_panel() {
    let file = json!({
        "filePath": "C:/repo/a.rs", "relativePath": "a.rs", "type": "update", "additions": 1, "deletions": 1,
        "before": "a\n", "after": "b\n", "diff": "@@ -1 +1 @@\n-a\n+b",
    });
    let patch = json!({
        "type": "tool", "tool": "apply_patch", "callID": "c1",
        "state": {
            "status": "completed", "input": { "patchText": "*** Begin Patch" }, "output": "Success.",
            "metadata": { "diff": "whole", "files": [file] },
        },
    });
    let read = json!({
        "type": "tool", "tool": "read", "callID": "c2",
        "state": {
            "status": "completed", "input": { "filePath": "a.rs" }, "output": "1: a",
            "metadata": { "display": "a", "preview": "a", "truncated": false },
        },
    });

    let parts = import_parts(&[patch, read]);

    let Part::ToolCall {
        metadata: Some(patched),
        ..
    } = &parts[0].part
    else {
        panic!("expected patch metadata");
    };
    assert_eq!(
        serde_json::to_value(patched).unwrap(),
        json!({
            "diff": "whole", "files": [{
                "filePath": "C:/repo/a.rs", "relativePath": "a.rs", "type": "update", "additions": 1,
                "deletions": 1, "patch": "@@ -1 +1 @@\n-a\n+b",
            }],
        })
    );

    let Part::ToolCall {
        metadata: Some(read),
        output,
        ..
    } = &parts[1].part
    else {
        panic!("expected read metadata");
    };
    assert_eq!(
        (serde_json::to_value(read).unwrap(), output.as_deref()),
        (json!({ "truncated": false }), Some("1: a")),
        "what the model read stays"
    );
}

#[test]
fn a_diff_too_big_for_any_panel_is_dropped_and_the_call_keeps_its_output() {
    let huge = "+x\n".repeat(400_000);
    let file = json!({ "filePath": "gen.rs", "additions": 400000, "deletions": 0, "diff": huge });
    let patch = json!({
        "type": "tool", "tool": "apply_patch", "callID": "c1",
        "state": {
            "status": "completed", "input": {}, "output": "Success.",
            "metadata": { "diff": huge, "files": [file] },
        },
    });

    let parts = import_parts(&[patch]);

    let Part::ToolCall {
        metadata: Some(metadata),
        output,
        ..
    } = &parts[0].part
    else {
        panic!("expected patch metadata");
    };
    assert_eq!(
        (serde_json::to_value(metadata).unwrap(), output.as_deref()),
        (
            json!({ "files": [{ "filePath": "gen.rs", "additions": 400000, "deletions": 0 }] }),
            Some("Success."),
        )
    );
}

#[test]
fn a_tool_call_stored_past_the_size_limit_keeps_its_name_input_and_output_and_nothing_else() {
    let huge = "x".repeat(9_000_000);
    let patch = json!({
        "type": "tool", "tool": "apply_patch", "callID": "c1",
        "state": {
            "status": "completed", "input": { "patchText": "*** Begin Patch" }, "output": "Success.",
            "title": "gen.rs", "metadata": { "diff": huge, "files": [{ "filePath": "gen.rs", "before": huge }] },
            "time": { "start": 5, "end": 6 },
        },
    });
    let image = json!({
        "type": "file", "mime": "image/png", "filename": "big.png", "url": format!("data:image/png;base64,{huge}"),
    });

    let parts = import_parts(&[patch, image]);

    let Part::ToolCall {
        call_id,
        name,
        input,
        status,
        output,
        title,
        metadata,
        started_at,
        finished_at,
    } = &parts[0].part
    else {
        panic!("{:?}", parts[0].part)
    };

    assert_eq!(
        (
            call_id.as_str(),
            name.as_str(),
            *status,
            output.as_deref(),
            title.as_deref()
        ),
        ("c1", "apply_patch", ToolStatus::Done, Some("Success."), Some("gen.rs"))
    );
    assert_eq!(
        (input, metadata, *started_at, *finished_at),
        (&json!({ "patchText": "*** Begin Patch" }), &None, Some(5), Some(6))
    );
    assert!(
        matches!(&parts[1].part, Part::File { url, .. } if url.len() > 9_000_000),
        "anything else that large is read whole"
    );
}
