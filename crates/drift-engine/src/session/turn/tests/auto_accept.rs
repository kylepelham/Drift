use super::*;

#[tokio::test]
async fn auto_accept_answers_every_ask_and_only_a_deny_rule_still_refuses() {
    let h = harness().await;
    asks_for(&h, "bash");
    let mut events = h.engine.hub.attach(None).rx;

    h.provider
        .push(tool_call("bash", r#"{"command": "echo inside"}"#))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("go")).await.await_ok();

    let waiting = next_ask(&mut events).await;
    assert_eq!(waiting.ask.pattern, "echo inside", "a rule asks");

    assert!(
        h.engine
            .set_session_auto_accept(&h.session.id, true)
            .unwrap()
            .unwrap()
            .auto_accept,
        "stored on the session"
    );

    until_idle(&h).await;

    assert_eq!(tool(&transcript(&h)[1].parts[0]).status, ToolStatus::Done);
    assert_auto_accept_policy(&h);
}

fn assert_auto_accept_policy(h: &Harness) {
    let ask = |ask: crate::tool::Ask| h.engine.permissions.decide_now(&h.session.id, &Policy::default(), &ask);
    let workspace = h._dir.join("ws");
    let bash = |line: &str| crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, line, line);
    assert_eq!(
        ask(crate::tool::Ask {
            default_allow: true,
            ..bash("git push")
        }),
        Decision::Allow,
        "what only a rule asks about"
    );
    assert_eq!(
        ask(crate::tool::Ask::path(
            "edit",
            &workspace.join("drift.json"),
            &workspace,
            "Edit"
        )),
        Decision::Allow,
        "a guarded workspace file"
    );
    assert_eq!(
        ask(crate::tool::Ask::path(
            "read",
            &workspace.join(".env"),
            &workspace,
            "Read"
        )),
        Decision::Allow,
        "a secret"
    );
    assert_eq!(
        ask(crate::tool::Ask::path(
            "edit",
            &h._dir.join("elsewhere.txt"),
            &workspace,
            "Edit"
        )),
        Decision::Allow,
        "outside the workspace"
    );
    assert_eq!(ask(bash("cat ../notes")), Decision::Allow, "a line reaching outside");
    assert_eq!(
        ask(crate::tool::Ask::shell(
            crate::tool::command::Dialect::Bash,
            "powershell.exe -Command \"Get-Process\"",
            "run"
        )),
        Decision::Allow,
        "a line that hides what it runs"
    );

    rule(h, "bash", "rm *", Decision::Deny);
    assert_eq!(
        ask(bash("rm -rf dist")),
        Decision::Deny,
        "a deny rule never asks, so auto-accept never answers it"
    );
    asks_for(h, "bash");
    h.engine.set_session_auto_accept(&h.session.id, false).unwrap();
    assert_eq!(
        ask(crate::tool::Ask {
            default_allow: true,
            ..bash("git push")
        }),
        Decision::Ask,
        "off again"
    );
    h.engine.set_auto_accept_all(true).unwrap();
    assert_eq!(
        ask(crate::tool::Ask {
            default_allow: true,
            ..bash("git push")
        }),
        Decision::Allow,
        "or on for every session"
    );
}
