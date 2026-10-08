use super::*;

#[tokio::test]
async fn a_projects_own_commands_run_only_once_the_user_says_so_and_always_holds_for_the_workspace() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    let marker = h._dir.join("ran.log");
    let (shell, flag, run) = if cfg!(windows) {
        ("cmd", "/c", format!("echo ran>> {}", marker.display()))
    } else {
        ("sh", "-c", format!("echo ran >> '{}'", marker.display()))
    };
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "checks": { "mark": { "command": [shell, flag, run], "extensions": [".txt"] } } }).to_string(),
    )
    .unwrap();
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("write", r#"{"path": "readme.md", "content": "r\n"}"#))
        .push(text("zero"));
    h.engine
        .submit(&h.session.id, prompt("write the readme"))
        .await
        .await_ok();
    until_idle(&h).await;

    assert!(
        h.engine.permissions.pending().is_empty(),
        "nothing of the project's covers a .md file, so nothing is asked"
    );

    h.provider
        .push(tool_call("write", r#"{"path": "a.txt", "content": "a\n"}"#))
        .push(text("one"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();

    let ask = next_ask(&mut events).await;
    assert_eq!(
        (ask.ask.kind.as_str(), ask.ask.pattern.as_str()),
        ("project-commands", format!("check mark: {shell} {flag} {run}").as_str())
    );

    reply_permission(&h, &ask.id, Reply::Deny);
    until_idle(&h).await;

    assert!(!marker.exists(), "refused, the project's command does not run");

    h.provider
        .push(tool_call("write", r#"{"path": "b.txt", "content": "b\n"}"#))
        .push(text("two"));
    h.engine.submit(&h.session.id, prompt("write b")).await.await_ok();
    until_idle(&h).await;

    assert!(
        h.engine.permissions.pending().is_empty() && !marker.exists(),
        "a refusal holds for the session without asking again"
    );
    assert_workspace_project_grants(&h, &mut events, &marker, (shell, flag, &run)).await;
}

async fn assert_workspace_project_grants(
    h: &Harness,
    events: &mut tokio::sync::broadcast::Receiver<crate::event::Envelope>,
    marker: &Path,
    command: (&str, &str, &str),
) {
    let (shell, flag, run) = command;
    let runs = || std::fs::read_to_string(marker).unwrap_or_default().lines().count();
    let other = sibling_session(h, "Other");
    h.provider
        .push(tool_call("write", r#"{"path": "c.txt", "content": "c\n"}"#))
        .push(text("three"));
    h.engine.submit(&other.id, prompt("write c")).await.await_ok();
    let ask = next_ask(events).await;
    reply_permission(h, &ask.id, Reply::Always);
    until_session_idle(h, &other.id).await;
    assert_eq!(runs(), 1, "allowed, it runs");

    let third = sibling_session(h, "Third");
    h.provider
        .push(tool_call("write", r#"{"path": "d.txt", "content": "d\n"}"#))
        .push(text("four"));
    h.engine.submit(&third.id, prompt("write d")).await.await_ok();
    until_session_idle(h, &third.id).await;
    assert_eq!(runs(), 2);
    assert!(
        h.engine.permissions.pending().is_empty(),
        "always holds for the workspace, in a new session too"
    );

    let changed = json!({
        "checks": {
            "mark": { "command": [shell, flag, format!("{run} & echo changed")], "extensions": [".txt"] },
        },
    });
    std::fs::write(h._dir.join("ws/drift.json"), changed.to_string()).unwrap();
    let fourth = sibling_session(h, "Fourth");
    h.provider
        .push(tool_call("write", r#"{"path": "e.txt", "content": "e\n"}"#))
        .push(text("five"));
    h.engine.submit(&fourth.id, prompt("write e")).await.await_ok();
    assert_eq!(
        next_ask(events).await.ask.kind,
        "project-commands",
        "changed commands are asked about again"
    );
    assert!(h.engine.abort(&fourth.id));
}

fn install_formatter(workspace: &Path, marker: &Path) {
    std::fs::create_dir_all(workspace.join("node_modules/.bin")).unwrap();
    std::fs::write(
        workspace.join("package.json"),
        r#"{ "devDependencies": { "prettier": "^3" } }"#,
    )
    .unwrap();

    let (shim, body) = if cfg!(windows) {
        ("prettier.cmd", format!("@echo ran>> \"{}\"\r\n", marker.display()))
    } else {
        ("prettier", format!("#!/bin/sh\necho ran >> '{}'\n", marker.display()))
    };
    std::fs::write(workspace.join("node_modules/.bin").join(shim), body).unwrap();

    #[cfg(unix)]
    std::fs::set_permissions(
        workspace.join("node_modules/.bin").join(shim),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

#[tokio::test]
async fn a_formatter_installed_in_the_project_runs_only_once_allowed() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    let marker = h._dir.join("ran.log");
    install_formatter(&h._dir.join("ws"), &marker);

    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("write", r#"{"path": "a.ts", "content": "let a = 1\n"}"#))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();

    let ask = next_ask(&mut events).await;
    assert_eq!(ask.ask.kind, "project-commands");
    assert!(
        ask.ask.pattern.starts_with("formatter prettier: ") && ask.ask.pattern.contains("node_modules"),
        "{}",
        ask.ask.pattern
    );

    reply_permission(&h, &ask.id, Reply::Deny);
    until_idle(&h).await;

    assert!(
        !marker.exists(),
        "a binary the repository brings never runs without the user's say-so"
    );
}

#[tokio::test]
async fn refusing_the_projects_formatter_leaves_its_checks_to_their_own_answer() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    let workspace = h._dir.join("ws");
    let (formatted, checked) = (h._dir.join("formatted.log"), h._dir.join("checked.log"));
    install_formatter(&workspace, &formatted);
    let (shell, flag, run) = if cfg!(windows) {
        ("cmd", "/c", format!("echo ran>> {}", checked.display()))
    } else {
        ("sh", "-c", format!("echo ran >> '{}'", checked.display()))
    };
    std::fs::write(
        workspace.join("drift.json"),
        json!({ "checks": { "mark": { "command": [shell, flag, run], "extensions": [".ts"] } } }).to_string(),
    )
    .unwrap();
    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("write", r#"{"path": "a.ts", "content": "let a = 1\n"}"#))
        .push(text("one"));
    h.engine.submit(&h.session.id, prompt("write a")).await.await_ok();

    let formatter = next_ask(&mut events).await;
    assert!(
        formatter.ask.pattern.starts_with("formatter prettier: "),
        "{}",
        formatter.ask.pattern
    );
    reply_permission(&h, &formatter.id, Reply::Deny);

    let check = next_ask(&mut events).await;
    assert!(
        check.ask.pattern.starts_with("check mark: "),
        "asked about apart from the formatter: {}",
        check.ask.pattern
    );

    reply_permission(&h, &check.id, Reply::Always);
    until_idle(&h).await;

    h.provider
        .push(tool_call("write", r#"{"path": "b.ts", "content": "let b = 1\n"}"#))
        .push(text("two"));
    h.engine.submit(&h.session.id, prompt("write b")).await.await_ok();
    until_idle(&h).await;

    assert!(h.engine.permissions.pending().is_empty());
    assert!(!formatted.exists(), "the refused formatter never runs");
    assert_eq!(
        std::fs::read_to_string(&checked).unwrap().lines().count(),
        2,
        "the allowed check runs after both writes"
    );
}

#[tokio::test]
async fn a_configured_formatter_runs_after_a_write() {
    let h = harness().await;
    allow_edits_and_project_commands(&h);
    let (program, rest) = if cfg!(windows) {
        ("cmd", r#""/c", "echo tidy> $FILE""#)
    } else {
        ("sh", r#""-c", "echo tidy > $FILE""#)
    };
    let config =
        format!(r#"{{ "formatters": {{ "tidy": {{ "command": ["{program}", {rest}], "extensions": [".txt"] }} }} }}"#);
    std::fs::write(h._dir.join("ws/drift.json"), config).unwrap();
    h.provider
        .push(tool_call("write", r#"{"path": "note.txt", "content": "raw\n"}"#))
        .push(text("written"));
    h.engine.submit(&h.session.id, prompt("write")).await.await_ok();
    until_idle(&h).await;

    assert!(
        std::fs::read_to_string(h._dir.join("ws/note.txt"))
            .unwrap()
            .starts_with("tidy")
    );
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.metadata.unwrap().formatted.as_ref().unwrap()[0], "tidy: note.txt");
    assert!(
        call.output
            .unwrap()
            .contains("A formatter then changed the result (tidy: note.txt)"),
        "the model is told: {:?}",
        call.output
    );

    h.provider
        .push(tool_call("write", r#"{"path": "note.txt", "content": "tidy\n"}"#))
        .push(text("again"));
    h.engine
        .submit(&h.session.id, prompt("write the same"))
        .await
        .await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let call = tool(&messages[messages.len() - 2].parts[0]);
    assert!(
        call.metadata.unwrap().formatted.is_none() && !call.output.unwrap().contains("formatter"),
        "a formatter that changed nothing is not mentioned"
    );
}
