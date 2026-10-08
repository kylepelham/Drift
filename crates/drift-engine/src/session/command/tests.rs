use crate::session::turn::tests::{Harness, harness, model, prompt, text, tool, until_idle};
use crate::session::types::ToolStatus;
use std::time::Duration;

use super::*;

fn select_model(h: &Harness) {
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&model()), None)
        .unwrap();
}

fn command(h: &Harness, name: &str, contents: &str) {
    let directory = h._dir.join("ws/.drift/commands");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(format!("{name}.md")), contents).unwrap();
}

#[tokio::test]
async fn a_commands_agent_and_model_run_that_turn_only() {
    let h = harness().await;
    select_model(&h);
    command(
        &h,
        "check",
        "---\nagent: plan\nmodel: anthropic/claude-haiku-4-5\nsubtask: false\n---\nReview $1 and $2.",
    );
    h.provider.push(text("reviewed")).push(text("next"));
    h.engine
        .execute_command(&h.session.id, "check", "src tests", None)
        .await
        .unwrap();
    until_idle(&h).await;

    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    assert_eq!(
        (
            session.agent.as_str(),
            session.model.as_ref().map(|model| model.model.as_str())
        ),
        ("build", Some(model().model.as_str())),
        "the session keeps its own"
    );
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(
        transcript[1].info.agent.as_deref(),
        Some("plan"),
        "the command's reply ran as its agent"
    );
    h.engine.submit(&h.session.id, prompt("carry on")).await.unwrap();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests[0].model, "claude-haiku-4-5");
    assert!(format!("{:?}", requests[0].messages).contains("Review src and tests."));
    assert_eq!(
        requests[1].model,
        model().model,
        "the next prompt is back on the session's model"
    );
}

#[tokio::test]
async fn shell_lines_run_as_checked_calls_and_at_files_are_mentioned() {
    let h = harness().await;
    select_model(&h);
    h.engine.permissions.set_policy(crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "bash".into(),
            pattern: "cargo publish*".into(),
            decision: crate::permission::Decision::Ask,
        }],
    });
    std::fs::write(h._dir.join("ws/NOTES.md"), "NOTE BODY").unwrap();
    command(
        &h,
        "brief",
        "Read @NOTES.md, then look at !`echo SHELL OUTPUT` and !`cargo publish`.",
    );
    h.provider.push(text("briefed"));
    let mut events = h.engine.hub.attach(None).rx;
    h.engine
        .execute_command(&h.session.id, "brief", "", None)
        .await
        .unwrap();
    let ask = loop {
        let envelope = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let crate::event::Event::PermissionAsked { request } = envelope.event {
            break request;
        }
    };
    assert_eq!(
        ask.ask.pattern, "cargo publish",
        "the line a rule asks about asks; the other ran without asking"
    );
    h.engine
        .permissions
        .reply(
            &h.engine.hub,
            &ask.id,
            crate::permission::ReplyBody {
                reply: crate::permission::Reply::Deny,
                pattern: None,
                message: None,
            },
        )
        .unwrap();
    until_idle(&h).await;

    let sent = format!("{:?}", h.provider.requests.lock().unwrap()[0].messages);
    assert!(
        sent.contains("`echo SHELL OUTPUT` (its output follows)")
            && sent.contains("ran `echo SHELL OUTPUT`, which returned:\\nSHELL OUTPUT"),
        "{sent}"
    );
    assert!(
        sent.contains("ran `cargo publish`, which failed"),
        "a refused line says so: {sent}"
    );
    assert!(sent.contains("NOTE BODY"), "@NOTES.md was read in as a mention");
    command(&h, "away", "---\nsubtask: true\n---\nCheck !`git status`.");
    assert!(matches!(
        h.engine.execute_command(&h.session.id, "away", "", None).await,
        Err(CommandError::Invalid(_))
    ));
}

#[tokio::test]
async fn a_prompt_steered_into_a_command_turn_is_answered_as_the_session_not_the_command() {
    let h = harness().await;
    select_model(&h);
    command(
        &h,
        "check",
        "---\nagent: plan\nmodel: anthropic/claude-haiku-4-5\n---\nReview it.",
    );
    h.provider
        .push_slow(Duration::from_millis(600), text("reviewed"))
        .push(text("edited"));
    h.engine
        .execute_command(&h.session.id, "check", "", None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut steered = prompt("now edit it");
    steered.model = None;
    h.engine.submit(&h.session.id, steered).await.unwrap();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(
        (requests[0].model.as_str(), requests[1].model.as_str()),
        ("claude-haiku-4-5", model().model.as_str()),
        "the steered prompt runs on the session's model"
    );
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(
        transcript.last().unwrap().info.agent.as_deref(),
        Some("build"),
        "and as the session's agent, not the command's read-only one"
    );
    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    assert_eq!(
        (session.agent.as_str(), session.model.unwrap().model),
        ("build", model().model)
    );
}

#[tokio::test]
async fn a_broken_primary_agent_cannot_be_picked_mid_turn() {
    let h = harness().await;
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/agents/hot.md"),
        "---\nmode: primary\npermission: { bash: often }\n---\nRuns hot.",
    )
    .unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/agents/warm.md"),
        "---\nmode: primary\ntop_p: 0.5\ntemperature: 0.9\n---\nRuns warm.",
    )
    .unwrap();
    let config = h.engine.workspace_config(&h._dir.join("ws"));
    assert!(
        config.agent("warm").unwrap().usable().is_ok(),
        "sampling fields are ignored, not refused"
    );
    assert!(
        config.warnings.iter().any(|warning| warning.contains("agent warm")
            && warning.contains("top_p")
            && warning.contains("temperature")),
        "{:?}",
        config.warnings
    );
    h.provider
        .push_slow(Duration::from_millis(600), text("busy"))
        .push(text("never"));
    h.engine.submit(&h.session.id, prompt("start")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut switch = prompt("as hot");
    switch.agent = Some("hot".into());
    let refused = h.engine.submit(&h.session.id, switch).await.unwrap_err();
    let TurnError::Config(reason) = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        reason.contains("agent hot") && reason.contains("allow, ask or deny"),
        "{refused:?}"
    );
    until_idle(&h).await;
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().agent, "build");
}

#[tokio::test]
async fn a_command_naming_a_subagent_always_delegates_and_broken_agents_are_refused_alone() {
    let h = harness().await;
    select_model(&h);
    std::fs::create_dir_all(h._dir.join("ws/.drift/agents")).unwrap();
    command(
        &h,
        "look",
        "---\nagent: explore\nsubtask: false\n---\nLook at $ARGUMENTS.",
    );
    std::fs::write(
        h._dir.join("ws/.drift/agents/hot.md"),
        "---\nmode: subagent\npermission: { read: sometimes }\n---\nRuns hot.",
    )
    .unwrap();
    command(&h, "heat", "---\nagent: hot\n---\nHeat $ARGUMENTS.");
    h.provider.push(text("found")).push(text("done"));
    h.engine
        .execute_command(&h.session.id, "look", "src", None)
        .await
        .unwrap();
    until_idle(&h).await;

    assert_eq!(h.engine.store.tasks_of(&h.session.id).unwrap()[0].agent, "explore");
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().agent, "build");
    let refused = h
        .engine
        .execute_command(&h.session.id, "heat", "src", None)
        .await
        .unwrap_err();
    let CommandError::Turn(TurnError::Config(reason)) = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        reason.contains("agent hot") && reason.contains("allow, ask or deny"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn subtask_commands_use_owned_foreground_workers_and_textual_parent_results() {
    let h = harness().await;
    select_model(&h);
    command(
        &h,
        "inspect",
        "---\nagent: explore\nmodel: anthropic/claude-haiku-4-5\nsubtask: true\n---\nInspect $ARGUMENTS.",
    );
    h.provider.push(text("worker answer")).push(text("parent answer"));
    h.engine
        .execute_command(&h.session.id, "inspect", "src", None)
        .await
        .unwrap();
    until_idle(&h).await;

    let tasks = h.engine.store.tasks_of(&h.session.id).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].agent, "explore");
    assert_eq!(tasks[0].mode, crate::session::tasks::Mode::Foreground);
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].model, "claude-haiku-4-5",
        "the command's model reaches the worker through metadata"
    );
    assert!(
        requests[0]
            .tools
            .iter()
            .chain(&requests[1].tools)
            .filter(|tool| tool.name == "task")
            .all(|tool| tool.input_schema["properties"].get("model").is_none()),
        "the model cannot pick one"
    );
    assert_eq!(requests[1].model, model().model);
    assert!(
        requests[1]
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .all(|block| !matches!(
                block,
                crate::llm::Block::ToolUse { .. } | crate::llm::Block::ToolResult { .. }
            ))
    );
    assert!(format!("{:?}", requests[1].messages).contains("worker answer"));
}

#[tokio::test]
async fn skill_commands_are_exposed_and_authorized_before_instructions_reach_the_model() {
    let h = harness().await;
    select_model(&h);
    std::fs::create_dir_all(h._dir.join("ws/.drift/skills/private")).unwrap();
    std::fs::write(
        h._dir.join("ws/.drift/skills/private/SKILL.md"),
        "---\nname: private\ndescription: A private skill\n---\nPRIVATE_INSTRUCTIONS for $ARGUMENTS.",
    )
    .unwrap();
    assert!(
        h.engine
            .workspace_config(&h._dir.join("ws"))
            .commands
            .iter()
            .any(|command| command.skill.as_deref() == Some("private"))
    );
    h.engine.permissions.set_policy(crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "skill".into(),
            pattern: "private".into(),
            decision: crate::permission::Decision::Deny,
        }],
    });
    h.provider.push(text("not loaded"));
    h.engine
        .execute_command(&h.session.id, "private", "src", None)
        .await
        .unwrap();
    until_idle(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(transcript.iter().flat_map(|message| &message.parts).any(|row| matches!(
        row.part,
        Part::ToolCall {
            status: ToolStatus::Denied,
            ..
        }
    )));
    assert!(!format!("{:?}", h.provider.requests.lock().unwrap()).contains("PRIVATE_INSTRUCTIONS"));

    h.engine.permissions.set_policy(crate::permission::Policy::default());
    h.provider.push(text("loaded"));
    h.engine
        .execute_command(&h.session.id, "private", "src", None)
        .await
        .unwrap();
    until_idle(&h).await;
    assert!(
        format!("{:?}", h.provider.requests.lock().unwrap().last().unwrap().messages)
            .contains("PRIVATE_INSTRUCTIONS for src.")
    );
    let call = tool(&transcript[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Denied);
}
