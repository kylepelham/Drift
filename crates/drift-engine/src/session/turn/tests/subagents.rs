use super::*;

#[tokio::test]
async fn a_task_runs_a_hidden_child_and_returns_its_reply() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    h.provider
        .push(tool_call(
            "task",
            r#"{"description": "Check a.txt", "prompt": "What is in a.txt?"}"#,
        ))
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(text("a.txt contains alpha"))
        .push(text("The subagent says alpha"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done);
    assert!(call.output.unwrap().starts_with("a.txt contains alpha\n\n(task_id:"));
    let child_id = call.metadata.unwrap().session_id.clone().unwrap();
    let child = h.engine.store.session(&child_id).unwrap().unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(h.session.id.as_str()));
    assert_eq!(child.visibility, Visibility::Hidden);
    assert_eq!(
        child.title, "Check a.txt (@general subagent)",
        "general takes a task when no type is given"
    );
    assert_eq!(h.engine.store.transcript(&child_id).unwrap().len(), 3);
    let listed = h
        .engine
        .store
        .sessions(crate::store::SessionFilter {
            workspace_id: None,
            archived: false,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert!(
        listed
            .iter()
            .any(|session| session.id == child_id && session.parent_id.as_deref() == Some(h.session.id.as_str())),
        "subagents are listed so the UI can nest them"
    );
}

#[tokio::test]
async fn a_finished_subagent_can_be_continued_with_what_it_already_saw() {
    let h = harness().await;
    h.provider
        .push(tool_call(
            "task",
            r#"{"description": "Find it", "prompt": "Where is FIRST_CONTEXT handled?"}"#,
        ))
        .push(text("in parser.rs"))
        .push(text("found"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    let (_, _, first) = task_call(&transcript(&h));
    let (task_id, child_id) = (
        first["taskId"].as_str().unwrap().to_string(),
        first["sessionId"].as_str().unwrap().to_string(),
    );
    let follow_up = json!({
        "description": "Follow up",
        "prompt": "And who calls it?",
        "task_id": task_id,
        "subagent_type": "explore",
    })
    .to_string();
    let mut again = tool_call("task", &follow_up);
    again[0] = Chunk::ToolUseStart {
        id: "toolu_task_2".into(),
        name: "task".into(),
    };
    h.provider.push(again).push(text("main.rs calls it")).push(text("done"));
    h.engine.submit(&h.session.id, prompt("ask again")).await.await_ok();
    until_idle(&h).await;

    let child_request = h.provider.requests.lock().unwrap().iter().rev().nth(1).unwrap().clone();
    let seen = format!("{:?}", child_request.messages);
    assert!(
        seen.contains("FIRST_CONTEXT") && seen.contains("in parser.rs") && seen.contains("who calls it"),
        "the subagent continues its own conversation: {seen}"
    );
    let tasks = h.engine.store.tasks_of(&h.session.id).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        (tasks[1].session_id.as_str(), tasks[1].agent.as_str()),
        (child_id.as_str(), "general"),
        "same transcript, same agent"
    );
    assert_eq!(
        h.engine.store.task_for_session(&child_id).unwrap().unwrap().id,
        tasks[1].id,
        "the latest task speaks for the session"
    );

    let mut bad = tool_call(
        "task",
        &json!({ "description": "x", "prompt": "y", "task_id": "task_nope" }).to_string(),
    );
    bad[0] = Chunk::ToolUseStart {
        id: "toolu_task_3".into(),
        name: "task".into(),
    };
    h.provider.push(bad).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("bad id")).await.await_ok();
    until_idle(&h).await;
    let messages = transcript(&h);
    let refused = messages
        .iter()
        .rev()
        .flat_map(|message| &message.parts)
        .find_map(|row| match &row.part {
            Part::ToolCall { output, .. } => output.clone(),
            _ => None,
        })
        .unwrap();
    assert!(refused.contains("no task task_nope"), "{refused}");
}

#[tokio::test]
async fn a_subagent_runs_on_its_agents_pinned_model_and_actions_are_not_agents() {
    let h = harness().await;
    let pinned = h.engine.catalog.read().unwrap().providers["anthropic"]
        .models
        .keys()
        .find(|id| id.as_str() != "claude-sonnet-4-5")
        .unwrap()
        .clone();
    let pin = crate::config::AgentOverride::from_json(&json!({ "model": format!("anthropic/{pinned}") }));
    h.engine
        .set_agent_overrides(std::collections::HashMap::from([("general".to_string(), pin)]));
    h.provider
        .push(tool_call("task", r#"{"description": "Pinned", "prompt": "go"}"#))
        .push(text("child done"))
        .push(tool_call(
            "task",
            r#"{"description": "Nope", "prompt": "go", "subagent_type": "title"}"#,
        ))
        .push(text("parent done"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(
        requests[0].model, "claude-sonnet-4-5",
        "the parent keeps the model it was prompted with"
    );
    assert_eq!(
        requests[1].model, pinned,
        "the subagent runs on the general agent's pin"
    );
    assert!(
        requests[1].system.contains("delegated job"),
        "and with the general agent's prompt"
    );
    assert!(
        requests[0].system.contains("# Subagents"),
        "the parent is told which subagents exist"
    );
    assert!(
        !requests[1].system.contains("# Subagents"),
        "a subagent cannot delegate, so it is not told"
    );
    let messages = transcript(&h);
    let refused = tool(&messages[2].parts[0]);
    assert_eq!(refused.status, ToolStatus::Error);
    assert!(refused.output.unwrap().contains("engine action"));
}

#[tokio::test]
async fn aborting_the_parent_aborts_a_running_child() {
    let h = harness().await;
    rule(&h, "bash", "*", Decision::Allow);
    let sleep = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    h.provider
        .push(tool_call("task", r#"{"description": "Wait", "prompt": "wait"}"#))
        .push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let child = child_id(&h);
    assert!(h.engine.turns.is_running(&child));
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    for _ in 0..100 {
        if !h.engine.turns.is_running(&child) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !h.engine.turns.is_running(&child),
        "the child must stop with its parent"
    );
}

#[tokio::test]
async fn subagents_are_not_offered_delegation_and_cannot_call_it() {
    let h = harness().await;
    h.provider
        .push(tool_call(
            "task",
            r#"{"description": "Nest", "prompt": "try to spawn"}"#,
        ))
        .push(tool_call("task", r#"{"description": "Sneaky", "prompt": "nest"}"#))
        .push(text("could not"))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    let names = |index: usize| {
        requests[index]
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>()
    };
    assert!(names(0).contains(&"task".to_string()) && !names(0).contains(&"spawn_thread".to_string()));
    for tool in crate::tool::task::DELEGATION {
        assert!(!names(1).iter().any(|name| name == tool), "subagent was offered {tool}");
    }
    let count: i64 = h
        .engine
        .store
        .lock()
        .query_row("SELECT COUNT(*) FROM session WHERE title LIKE 'Sneaky%'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    let child = child_id(&h);
    assert_eq!(
        tool(&h.engine.store.transcript(&child).unwrap()[1].parts[0]).status,
        ToolStatus::Error
    );
}
