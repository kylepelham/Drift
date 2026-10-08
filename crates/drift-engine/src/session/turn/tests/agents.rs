use super::*;

#[tokio::test]
async fn leaving_plan_tells_the_model_once_that_it_may_now_change_files() {
    let h = harness().await;
    let reminded = |request: &Request| {
        format!("{:?}", request.messages).contains("switched from the plan agent to the build agent")
    };
    h.provider
        .push(text("the plan"))
        .push(text("building"))
        .push(text("more"));
    h.engine
        .submit(
            &h.session.id,
            Prompt {
                agent: Some("plan".into()),
                ..prompt("plan it")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;
    h.engine
        .submit(
            &h.session.id,
            Prompt {
                agent: Some("build".into()),
                ..prompt("go")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("and more")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(
        requests.iter().map(reminded).collect::<Vec<_>>(),
        [false, true, true],
        "from the turn that left plan on"
    );
    let last = format!("{:?}", requests[2].messages);
    assert_eq!(
        last.matches("switched from the plan agent").count(),
        1,
        "once, on the prompt that left plan, so the cached prefix stays the same"
    );
    let stored = serde_json::to_string(&transcript(&h)).unwrap();
    assert!(!stored.contains("switched from the plan agent"), "never stored");
}

#[tokio::test]
async fn a_prompt_that_picks_plan_runs_as_plan_and_every_message_says_so() {
    let h = harness().await;
    h.provider.push(text("planned")).push(text("still planning"));
    h.engine
        .submit(
            &h.session.id,
            Prompt {
                agent: Some("plan".into()),
                ..prompt("plan it")
            },
        )
        .await
        .await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("and then")).await.await_ok();
    until_idle(&h).await;

    let reminded: Vec<_> = h
        .provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| format!("{:?}", request.messages).matches("# Plan mode").count())
        .collect();
    assert_eq!(
        reminded,
        [1, 1],
        "the prompt that started planning carries plan's reminder, kept as it was sent; the next plan turn adds none"
    );
    assert_eq!(session(&h).agent, "plan", "a prompt that names none keeps it");
    let agents: Vec<_> = transcript(&h).into_iter().map(|message| message.info.agent).collect();
    assert_eq!(agents, vec![Some("plan".to_string()); 4]);
    for refused in ["explore", "nobody"] {
        let result = h
            .engine
            .submit(
                &h.session.id,
                Prompt {
                    agent: Some(refused.into()),
                    ..prompt("x")
                },
            )
            .await;

        assert_eq!(
            result.err(),
            Some(TurnError::UnknownAgent),
            "{refused} cannot run a conversation"
        );
    }
}

#[tokio::test]
async fn a_drift_json_that_cannot_be_read_stops_the_turn_instead_of_dropping_its_rules() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/drift.json"), r#"{ "permissions": [ "#).unwrap();
    let refused = h.engine.submit(&h.session.id, prompt("run")).await.err();
    assert!(
        matches!(&refused, Some(TurnError::Config(problem)) if problem.contains("drift.json could not be read")),
        "{refused:?}"
    );
    assert!(transcript(&h).is_empty(), "nothing was admitted");
}

#[tokio::test]
async fn workspace_config_shapes_the_turn() {
    let h = harness().await;
    let workspace = h._dir.join("ws");
    std::fs::write(
        workspace.join("drift.json"),
        r#"{ "permissions": [{ "kind": "bash", "pattern": "echo *", "decision": "deny" }] }"#,
    )
    .unwrap();
    std::fs::create_dir_all(workspace.join(".drift/skills/tidy")).unwrap();
    std::fs::write(
        workspace.join(".drift/skills/tidy/SKILL.md"),
        "---\ndescription: Tidies\n---\nTidy up.",
    )
    .unwrap();
    h.provider
        .push(tool_call("bash", r#"{"command": "echo hi"}"#))
        .push(text("denied"));
    h.engine.submit(&h.session.id, prompt("run")).await.await_ok();
    until_idle(&h).await;
    let messages = transcript(&h);
    assert_eq!(tool(&messages[1].parts[0]).status, ToolStatus::Denied);
    let system = h.provider.requests.lock().unwrap()[0].system.clone();
    assert!(
        system.contains("- tidy: Tidies"),
        "skills are listed in the system prompt"
    );

    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("plan"))
        .unwrap();
    h.provider.push(text("planned"));
    h.engine.submit(&h.session.id, prompt("plan it")).await.await_ok();
    until_idle(&h).await;
    let requests = h.provider.requests.lock().unwrap().clone();
    let (build, plan) = (&requests[0], requests.last().unwrap());
    assert_eq!((&plan.system, &plan.tools), (&build.system, &build.tools));
    assert!(format!("{:?}", plan.messages).contains("# Plan mode") && !plan.system.contains("# Plan mode"));
}

#[tokio::test]
async fn a_read_only_agent_never_calls_an_untrusted_servers_mcp_tool_even_one_it_calls_read_only() {
    let h = harness().await;
    let config = echo_server();
    h.engine.store.save_mcp_server("echo", &config).unwrap();
    h.engine.store.set_mcp_read_only_trusted("echo", false).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("plan"))
        .unwrap();
    h.provider
        .push(tool_call("echo_echo", r#"{"text": "hi"}"#))
        .push(text("noted"));
    h.engine.submit(&h.session.id, prompt("echo")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Error);
    assert!(
        call.output.unwrap().contains("only reads"),
        "the server's read-only mark is its own claim: {:?}",
        call.output
    );
}

#[tokio::test]
async fn a_read_only_agent_is_refused_every_call_that_would_change_something() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![
            Rule {
                kind: "edit".into(),
                pattern: "*".into(),
                decision: Decision::Allow,
            },
            Rule {
                kind: "bash".into(),
                pattern: "*".into(),
                decision: Decision::Allow,
            },
        ],
    });
    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("plan"))
        .unwrap();
    h.provider
        .push(
            [
                call_block("t1", "write", r#"{"path": "plan-mutated.txt", "content": "x\n"}"#),
                call_block("t2", "bash", r#"{"command": "echo x > made.txt"}"#),
                call_block(
                    "t3",
                    "task",
                    r#"{"description": "Change it", "prompt": "edit a file", "subagent_type": "general"}"#,
                ),
                call_block("t4", "bash", r#"{"command": "git status"}"#),
                vec![Chunk::Stop(StopReason::ToolUse)],
            ]
            .concat(),
        )
        .push(text("noted"));
    h.engine
        .submit(&h.session.id, prompt("write it anyway"))
        .await
        .await_ok();
    until_idle(&h).await;

    assert!(
        !h._dir.join("ws/plan-mutated.txt").exists() && !h._dir.join("ws/made.txt").exists(),
        "plan mode must not write"
    );
    let messages = transcript(&h);
    for row in &messages[1].parts[..3] {
        let refused = tool(row);
        assert_eq!(refused.status, ToolStatus::Error);
        assert!(refused.output.unwrap().contains("only reads"), "{:?}", refused.output);
        assert!(
            refused.metadata.is_none(),
            "nothing was recorded for a call that never ran"
        );
    }
    assert_eq!(
        tool(&messages[1].parts[3]).status,
        ToolStatus::Done,
        "a shell line that only reads runs, so plan can look at git history"
    );
    let sessions = h
        .engine
        .store
        .sessions(crate::store::SessionFilter {
            workspace_id: None,
            archived: false,
            before: None,
            limit: 10,
        })
        .unwrap();
    assert_eq!(sessions.len(), 1, "no writing subagent was started");
}
