use super::*;
use crate::session::tasks::{Mode, TaskState};

#[tokio::test]
async fn tasks_are_listed_read_and_stopped_and_background_can_be_turned_off() {
    let harness = harness().await;
    let (_, session_id) = session_with_model(&harness).await;
    let listed = json_response(harness.get(&format!("/sessions/{session_id}/tasks"))).await;
    assert_eq!(listed, json!([]));
    let parent = harness.engine.store.session(&session_id).unwrap().unwrap();
    let new = crate::store::tasks::tests::new_task(&session_id, "c", Mode::Background);
    let task = harness
        .engine
        .store
        .launch_task(new, crate::store::tasks::tests::child(&parent))
        .unwrap()
        .task;

    let read = json_response(harness.get(&format!("/tasks/{}", task.id))).await;
    assert_eq!(
        (read["state"].as_str(), read["mode"].as_str()),
        (Some("queued"), Some("background"))
    );
    let stopped = json_response(harness.post(&format!("/tasks/{}/abort", task.id))).await;
    assert_eq!(stopped["state"], "stopped", "a queued worker never starts");
    assert_eq!(
        harness.engine.store.task(&task.id).unwrap().unwrap().state,
        TaskState::Stopped
    );
    assert_eq!(response_status(harness.get("/tasks/task_nope")).await, 404);

    let settings = json_response(harness.get("/settings")).await;
    assert_eq!(settings["backgroundTasks"], true);
    let off = json_response(
        harness
            .put("/settings")
            .json(&json!({ "autoCompact": true, "backgroundTasks": false })),
    )
    .await;
    assert_eq!(off["backgroundTasks"], false);
    let kept = json_response(harness.put("/settings").json(&json!({ "autoCompact": false }))).await;
    assert_eq!(
        (kept["autoCompact"].as_bool(), kept["backgroundTasks"].as_bool()),
        (Some(false), Some(false)),
        "left out, it stays"
    );
}

#[tokio::test]
async fn base_prompts_are_replaced_for_every_model_or_one_family_and_the_shared_rules_stay() {
    let harness = harness().await;
    let provider = script_replies(&harness, "ok", 3);
    harness
        .put("/providers/anthropic/key")
        .json(&json!({ "key": "k" }))
        .send()
        .await
        .unwrap();
    let (_, session_id) = session_with_model(&harness).await;
    let listed = json_response(harness.get("/prompts")).await;
    let ids: Vec<&str> = listed["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|prompt| prompt["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["all", "codex", "claude", "gemini", "default"]);
    assert!(
        listed["prompts"][2]["default"]
            .as_str()
            .unwrap()
            .starts_with("You are Drift")
            && listed["shared"].as_str().unwrap().contains("<system-reminder>")
    );
    let system = |index: usize| provider.requests.lock().unwrap()[index].system.clone();

    assert_eq!(
        response_status(
            harness
                .put("/prompts/all")
                .json(&json!({ "text": "Every model works my way." }))
        )
        .await,
        200
    );
    submit_and_wait(&harness, &session_id, "one").await;
    assert!(
        system(0).starts_with("Every model works my way.\n\n# Tools") && system(0).contains("<system-reminder>"),
        "{}",
        system(0)
    );

    harness
        .put("/prompts/claude")
        .json(&json!({ "text": "Claude works this way." }))
        .send()
        .await
        .unwrap();
    submit_and_wait(&harness, &session_id, "two").await;
    assert!(
        system(1).starts_with("Claude works this way."),
        "a family's own wins over the one for every model"
    );
    let reset = json_response(harness.delete("/prompts/claude")).await;
    assert!(reset["prompts"][2].get("custom").is_none());
    harness.delete("/prompts/all").send().await.unwrap();
    submit_and_wait(&harness, &session_id, "three").await;
    assert!(system(2).starts_with("You are Drift"), "reset, Drift's own is back");

    assert_eq!(
        response_status(harness.put("/prompts/claude").json(&json!({ "text": "  " }))).await,
        400
    );
    assert_eq!(
        response_status(harness.put("/prompts/nope").json(&json!({ "text": "x" }))).await,
        404
    );
}
