use super::*;

fn write_call(path: &str, content: &str) -> Vec<Chunk> {
    tool_call("write", &json!({ "path": path, "content": content }).to_string())
}

/// Each tool call's status and output, in order.
fn calls(h: &Harness) -> Vec<(ToolStatus, String)> {
    transcript(h)
        .iter()
        .flat_map(|message| message.parts.iter())
        .filter(|part| matches!(part.part, Part::ToolCall { .. }))
        .map(|part| {
            let view = tool(part);
            (view.status, view.output.unwrap_or_default().to_string())
        })
        .collect()
}

#[tokio::test]
async fn plan_writes_its_plan_is_refused_elsewhere_and_build_reads_it_after_a_switch() {
    let h = harness().await;
    let plans = h.engine.plans_dir(&h.session.workspace_id);
    let plan_file = plans.join("auth.md");
    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("plan"))
        .unwrap();
    h.provider
        .push(write_call(&plan_file.to_string_lossy(), "# Auth plan\n"))
        .push(write_call("src.txt", "x"))
        .push(write_call(&plans.join("notes.txt").to_string_lossy(), "x"))
        .push(text("planned"));
    h.engine
        .submit(&h.session.id, prompt("write the plan down"))
        .await
        .await_ok();
    until_idle(&h).await;

    assert!(
        h.engine.permissions.pending().is_empty(),
        "a plan file is written unasked"
    );
    assert_eq!(std::fs::read_to_string(&plan_file).unwrap(), "# Auth plan\n");
    let written = calls(&h);
    assert_eq!(written[0].0, ToolStatus::Done);
    for refused in &written[1..3] {
        assert_eq!(refused.0, ToolStatus::Error);
        let output = &refused.1;
        assert!(
            output.contains(&plans.display().to_string()) && output.contains("nowhere else"),
            "{output}"
        );
    }
    assert!(!h._dir.join("ws/src.txt").exists() && !plans.join("notes.txt").exists());
    let system = h.provider.requests.lock().unwrap()[0].system.clone();
    assert!(
        system.contains(&format!("Plans directory: {}", plans.display())),
        "{system}"
    );

    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("build"))
        .unwrap();
    h.provider
        .push(tool_call(
            "read",
            &json!({ "path": plan_file.to_string_lossy() }).to_string(),
        ))
        .push(text("read it"));
    h.engine.submit(&h.session.id, prompt("carry it out")).await.await_ok();
    until_idle(&h).await;

    let read = calls(&h).pop().unwrap();
    assert_eq!(read.0, ToolStatus::Done, "build reads the plan without asking");
    assert!(read.1.contains("# Auth plan"));
}

#[tokio::test]
async fn after_a_summary_the_request_names_the_plan_files_in_a_steady_order() {
    let h = harness().await;
    let plans = h.engine.plans_dir(&h.session.workspace_id);
    assert_eq!(h.engine.plans_note(&h.session.workspace_id), None, "no plans, no note");
    std::fs::create_dir_all(&plans).unwrap();
    std::fs::write(plans.join("b.md"), "b").unwrap();
    std::fs::write(plans.join("a.md"), "a").unwrap();
    std::fs::write(plans.join("skip.txt"), "x").unwrap();

    let note = h.engine.plans_note(&h.session.workspace_id).unwrap();
    let (a, b) = (
        plans.join("a.md").display().to_string(),
        plans.join("b.md").display().to_string(),
    );
    assert!(note.find(&a).unwrap() < note.find(&b).unwrap(), "{note}");
    assert!(!note.contains("skip.txt"));
}
