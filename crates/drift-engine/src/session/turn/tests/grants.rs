use super::*;

#[tokio::test]
async fn always_holds_for_the_workspace_across_sessions_and_restarts_and_settles_asks_it_covers() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;
    let other = sibling_session(&h, "Other");
    h.provider
        .push(tool_call("bash", r#"{"command": "cargo --version"}"#))
        .push(tool_call("bash", r#"{"command": "cargo --version"}"#))
        .push(text("one"))
        .push(text("two"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    let first = next_ask(&mut events).await;
    h.engine.submit(&other.id, prompt("second")).await.await_ok();

    let second = next_ask(&mut events).await;
    assert_ne!(first.session_id, second.session_id);

    reply_permission(&h, &first.id, Reply::Always);
    until_idle(&h).await;

    for _ in 0..300 {
        if !h.engine.turns.is_running(&other.id) {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        h.engine.permissions.pending().is_empty(),
        "the other session's waiting ask was answered by the same grant"
    );
    assert_eq!(h.provider.responses_left(), 0);
    assert_workspace_grants_survive_restart(&h);
}

fn assert_workspace_grants_survive_restart(h: &Harness) {
    let reopened = Engine::open_with(
        &h._dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    reopened.bind_permissions("ses_later", &h.session.workspace_id);
    let ask = crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, "cargo --version", "");
    assert_eq!(
        reopened.permissions.decide_now("ses_later", &Policy::default(), &ask),
        Decision::Allow,
        "kept for the workspace across a restart"
    );
    reopened.bind_permissions("ses_elsewhere", "another-workspace");
    assert_eq!(
        reopened
            .permissions
            .decide_now("ses_elsewhere", &Policy::default(), &ask),
        Decision::Ask,
        "and only for that workspace"
    );
    let deny = Policy {
        rules: vec![Rule {
            kind: "bash".into(),
            pattern: "cargo *".into(),
            decision: Decision::Deny,
        }],
    };
    assert_eq!(
        reopened.permissions.decide_now("ses_later", &deny, &ask),
        Decision::Deny,
        "a deny rule added later beats the kept grant"
    );

    let grants = reopened.permission_grants(&h.session.workspace_id);
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert!(reopened.revoke_permission_grant(&h.session.workspace_id, Some(&grants[0])));
    assert!(
        !reopened.revoke_permission_grant(&h.session.workspace_id, Some(&grants[0])),
        "already gone"
    );
    assert_eq!(
        reopened.permissions.decide_now("ses_later", &Policy::default(), &ask),
        Decision::Ask,
        "revoked, it asks again"
    );
    let after_restart = Engine::open_with(
        &h._dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        after_restart.permission_grants(&h.session.workspace_id).is_empty(),
        "the revoke was stored"
    );
}

#[tokio::test]
async fn a_subagent_runs_under_its_parents_approvals() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("bash", r#"{"command": "cargo --version"}"#))
        .push(tool_call(
            "task",
            r#"{"description": "Check", "prompt": "check the toolchain"}"#,
        ))
        .push(tool_call("bash", r#"{"command": "cargo --version"}"#))
        .push(text("child done"))
        .push(text("parent done"));
    h.engine.submit(&h.session.id, prompt("check")).await.await_ok();
    let ask = next_ask(&mut events).await;
    reply_permission(&h, &ask.id, Reply::Always);
    until_idle(&h).await;

    assert_eq!(
        h.provider.responses_left(),
        0,
        "the child's same command ran without asking"
    );
    assert!(h.engine.permissions.pending().is_empty());
}
