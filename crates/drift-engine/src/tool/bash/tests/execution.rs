use super::super::output::{SHOW_EVERY, collect, without_blank_ends};
use super::*;

#[test]
fn output_loses_blank_lines_at_its_ends_but_never_its_first_lines_indentation() {
    assert_eq!(
        without_blank_ends("\n\r\n  @@ -1 +1 @@\n-a\n+b\n\n"),
        "  @@ -1 +1 @@\n-a\n+b"
    );
    assert_eq!(without_blank_ends("   \n\t\n"), "");
    assert_eq!(without_blank_ends("plain"), "plain");
}

#[tokio::test]
async fn runs_a_command_in_the_workspace_and_reports_exit_codes() {
    let sandbox = Sandbox::new("bash");
    sandbox.file("hello.txt", "hi");
    let bash = Bash::detect();
    let list = match bash.shell {
        Shell::Bash(_) => "ls && exit 3",
        Shell::PowerShell(_) => "Get-ChildItem -Name; exit 3",
    };

    let output = bash
        .run(&sandbox.ctx, json!({ "command": list, "description": "list files" }))
        .await
        .unwrap();
    assert!(output.output.contains("hello.txt"), "{}", output.output);
    assert!(output.output.ends_with("exit code 3"));
    assert_eq!(
        output.metadata.notes,
        Some(vec!["exit code 3".to_string()]),
        "said by Drift, not printed by the command"
    );
    assert_eq!(output.title, "list files");
    assert_eq!(output.metadata.exit, Some(3));
}

#[tokio::test]
async fn a_workdir_is_where_it_runs_and_one_outside_the_workspace_asks() {
    let sandbox = Sandbox::new("bash-workdir");
    let file = sandbox.file("sub/here.txt", "here\n");
    let bash = Bash::detect();
    let print = match bash.shell {
        Shell::Bash(_) => "cat here.txt",
        Shell::PowerShell(_) => "Get-Content here.txt",
    };

    let output = bash
        .run(&sandbox.ctx, json!({ "command": print, "workdir": "sub" }))
        .await
        .unwrap();
    assert!(output.output.contains("here"), "{}", output.output);
    assert!(
        sandbox.ctx.files.was_read(&file),
        "a file it printed is found from the workdir"
    );

    let parent = crate::tool::canonical(sandbox.ctx.workspace.parent().unwrap());
    let enter = format!("cd {}", parent.display());
    let outside = bash
        .ask(&sandbox.ctx, &json!({ "command": "git log", "workdir": ".." }))
        .unwrap();
    assert_eq!(outside.reason, Some(Reason::Moves));
    assert_eq!(outside.pattern, format!("{enter} && git log"));
    assert_eq!(
        outside.commands.unwrap(),
        [enter, "git log".to_string()],
        "the move is approved with the line, never by an earlier approval of git log alone"
    );
    bash.run(&sandbox.ctx, json!({ "command": "exit 0", "workdir": ".." }))
        .await
        .expect("an approved workdir outside the workspace runs");

    let ask = bash
        .ask(
            &sandbox.ctx,
            &json!({ "command": "cd .. && cargo test", "workdir": "sub" }),
        )
        .unwrap();
    assert_eq!(
        ask.commands.unwrap(),
        ["cargo test"],
        "a move from the workdir that stays inside asks nothing"
    );
}

#[tokio::test]
async fn a_file_printed_by_a_command_that_succeeded_counts_as_read() {
    let sandbox = Sandbox::new("bash-reads");
    let shown = sandbox.file("shown.txt", "one\n");
    let missed = sandbox.file("missed.txt", "two\n");
    let bash = Bash::detect();
    let (print, fail) = match bash.shell {
        Shell::Bash(_) => ("cat shown.txt", "cat missed.txt && exit 1"),
        Shell::PowerShell(_) => ("Get-Content shown.txt", "Get-Content missed.txt; exit 1"),
    };

    bash.run(&sandbox.ctx, json!({ "command": print })).await.unwrap();
    bash.run(&sandbox.ctx, json!({ "command": fail })).await.unwrap();
    assert!(sandbox.ctx.files.was_read(&shown), "edit may follow a shell read");
    assert!(
        !sandbox.ctx.files.was_read(&missed),
        "a line that failed is not trusted to have shown it"
    );
}

/// When each output update was shown, for a command run as `node -e <script>`.
async fn shows_for(script: &str) -> Vec<std::time::Instant> {
    let mut child = tokio::process::Command::new("node")
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let shown = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = shown.clone();
    let progress = Progress::new(move |_| record.lock().unwrap().push(std::time::Instant::now()));
    let mut spool = Spool::new(None);
    let mut pipes = crate::background::Pipes::take(&mut child);

    collect(&mut child, &mut pipes, &mut spool, &progress).await;
    shown.lock().unwrap().clone()
}

#[tokio::test]
async fn a_noisy_command_shows_its_output_at_most_every_show_every_and_a_quiet_one_once() {
    let noisy = shows_for(
        "let i = 0; const t = setInterval(() => { console.log('line ' + i++); if (i >= 60) clearInterval(t) }, 25)",
    )
    .await;
    assert!(
        (2..=5).contains(&noisy.len()),
        "about 1.5 s of output, shown every 500 ms: {} times",
        noisy.len()
    );
    for pair in noisy.windows(2) {
        assert!(
            pair[1] - pair[0] >= SHOW_EVERY - Duration::from_millis(50),
            "two updates {:?} apart",
            pair[1] - pair[0]
        );
    }

    let quiet = shows_for("console.log('once'); setTimeout(() => {}, 1600)").await;
    assert_eq!(
        quiet.len(),
        1,
        "output that stopped growing is not shown again on every tick"
    );
}

#[tokio::test]
async fn a_command_at_its_limit_moves_to_the_background() {
    let sandbox = Sandbox::new("bash-timeout");
    let bash = Bash::detect();
    let sleep = match bash.shell {
        Shell::Bash(_) => "sleep 5",
        Shell::PowerShell(_) => "Start-Sleep 5",
    };
    let early_then_sleep = match bash.shell {
        Shell::Bash(_) => "echo early; sleep 2; echo late",
        Shell::PowerShell(_) => "Write-Output early; Start-Sleep 2; Write-Output late",
    };

    let moved = bash
        .run(&sandbox.ctx, json!({ "command": early_then_sleep, "timeout": 1000 }))
        .await
        .unwrap();
    assert!(!bash.failed(&moved), "a command that goes on is not a failed call");
    assert_eq!(moved.metadata.shell_timeout_ms.flatten(), Some(1000));
    assert!(
        moved.output.starts_with("early"),
        "what it printed before the limit is kept: {}",
        moved.output
    );
    assert!(moved.output.contains("moved to the background"), "{}", moved.output);

    // It goes on where it was, and the next read starts after what the call returned.
    let id = moved.metadata.process_id.clone().unwrap();
    let handle = sandbox.ctx.engine.processes.get(&id).unwrap();
    let never = tokio_util::sync::CancellationToken::new();
    handle.wait(None, Duration::from_secs(20), &never).await;
    assert_eq!(handle.exit(), Some(crate::background::Exit::Code(0)));
    let rest = handle.unread().text;
    assert!(rest.contains("late") && !rest.contains("early"), "{rest}");

    sandbox.ctx.engine.set_shell_timeout(Some(Duration::from_millis(300)));
    assert_eq!(
        bash.running_metadata(&sandbox.ctx, &json!({ "command": sleep }))
            .unwrap()
            .shell_timeout_ms,
        Some(Some(300))
    );
    let by_setting = bash.run(&sandbox.ctx, json!({ "command": sleep })).await.unwrap();
    assert!(
        by_setting.metadata.process_id.is_some(),
        "without a `timeout` the Settings limit applies"
    );
    let asked_longer = json!({ "command": sleep, "timeout": 3_600_000 });
    assert_eq!(
        bash.running_metadata(&sandbox.ctx, &asked_longer)
            .unwrap()
            .shell_timeout_ms,
        Some(Some(300)),
        "a longer `timeout` cannot hold a command in front past the Settings limit"
    );
    let capped = bash.run(&sandbox.ctx, asked_longer).await.unwrap();
    assert!(capped.metadata.process_id.is_some());
    sandbox.ctx.engine.processes.stop_session(&sandbox.ctx.session_id);

    sandbox.ctx.engine.set_shell_timeout(None);
    let quick = match bash.shell {
        Shell::Bash(_) => "sleep 1",
        Shell::PowerShell(_) => "Start-Sleep 1",
    };
    let unlimited = bash.run(&sandbox.ctx, json!({ "command": quick })).await.unwrap();
    assert!(
        !bash.failed(&unlimited) && unlimited.metadata.shell_timeout_ms == Some(None),
        "no limit lets it finish"
    );
}

#[tokio::test]
async fn a_background_command_returns_at_once_and_one_that_fails_at_once_says_so() {
    let sandbox = Sandbox::new("bash-background");
    let bash = Bash::detect();
    let (server, broken) = match bash.shell {
        Shell::Bash(_) => ("echo listening on http://localhost:4321; sleep 30", "echo nope; exit 2"),
        Shell::PowerShell(_) => (
            "Write-Output 'listening on http://localhost:4321'; Start-Sleep 30",
            "Write-Output nope; exit 2",
        ),
    };

    let started = std::time::Instant::now();
    let running = bash
        .run(&sandbox.ctx, json!({ "command": server, "background": true }))
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(running.metadata.process_id.is_some());
    assert!(running.output.contains("runs in the background"), "{}", running.output);

    let failed = bash
        .run(&sandbox.ctx, json!({ "command": broken, "background": true }))
        .await
        .unwrap();
    assert_eq!(failed.metadata.exit, Some(2), "{}", failed.output);
    assert!(failed.output.starts_with("nope"), "{}", failed.output);

    sandbox.ctx.engine.processes.stop_session(&sandbox.ctx.session_id);
}

#[tokio::test]
async fn stop_keeps_what_the_command_printed() {
    let sandbox = Sandbox::new("bash-stop");
    let bash = Bash::detect();
    let command = match bash.shell {
        Shell::Bash(_) => "echo early; sleep 30",
        Shell::PowerShell(_) => "Write-Output early; Start-Sleep 30",
    };
    let context = sandbox.ctx_clone();
    let abort = context.abort.clone();
    let running = tokio::spawn(async move { Bash::detect().run(&context, json!({ "command": command })).await });
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let started = std::time::Instant::now();

    abort.cancel();
    let output = running.await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3), "Stop is prompt");
    assert!(bash.failed(&output) && output.metadata.stopped == Some(true));
    assert!(
        output.output.starts_with("early") && output.output.contains("stopped the command"),
        "{}",
        output.output
    );
}

#[tokio::test]
async fn a_background_process_holding_the_output_does_not_keep_the_call_waiting() {
    let sandbox = Sandbox::new("bash-lingering");
    sandbox.ctx.engine.set_shell_timeout(None);
    let bash = Bash::detect();
    let command = match bash.shell {
        Shell::Bash(_) => "sleep 30 & echo done",
        Shell::PowerShell(_) => {
            "Start-Process -NoNewWindow pwsh -ArgumentList '-NoProfile','-Command','Start-Sleep 30'; Write-Output done"
        }
    };
    let started = std::time::Instant::now();

    let output = bash.run(&sandbox.ctx, json!({ "command": command })).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "no limit, yet it returns: {:?}",
        started.elapsed()
    );
    assert!(output.output.starts_with("done"), "{}", output.output);
    assert_eq!(output.metadata.exit, Some(0));
    assert!(!bash.failed(&output));
    if matches!(bash.shell, Shell::Bash(_)) {
        assert!(
            output
                .output
                .contains("Background processes still held the output open"),
            "{}",
            output.output
        );
    }
}

#[tokio::test]
async fn large_output_is_bounded_in_the_result_and_whole_on_disk() {
    let sandbox = Sandbox::new("bash-large");
    let bash = Bash::detect();
    let command = match bash.shell {
        Shell::Bash(_) => "for i in $(seq 1 40000); do echo line $i; done",
        Shell::PowerShell(_) => "1..40000 | ForEach-Object { \"line $_\" }",
    };

    let output = bash.run(&sandbox.ctx, json!({ "command": command })).await.unwrap();
    assert!(
        output.output.len() < crate::tool::spool::HEAD_BYTES + crate::tool::spool::TAIL_BYTES + 400,
        "{}",
        output.output.len()
    );
    assert!(output.output.starts_with("line 1") && output.output.contains("line 40000"));
    let file = output
        .metadata
        .output_file
        .as_deref()
        .expect("the whole output is kept");
    let whole = std::fs::read_to_string(file).unwrap();
    assert_eq!(whole.lines().count(), 40000);
    assert_eq!(
        output.metadata.output_bytes,
        Some(std::fs::metadata(file).unwrap().len())
    );
}

#[tokio::test]
async fn with_hand_off_off_a_command_at_its_limit_is_stopped_as_before() {
    let sandbox = Sandbox::new("bash-no-handoff");
    sandbox
        .ctx
        .engine
        .store
        .set_setting(crate::background::HANDOFF_KEY, &false)
        .unwrap();
    let bash = Bash::detect();
    let line = match bash.shell {
        Shell::Bash(_) => "echo early; sleep 5",
        Shell::PowerShell(_) => "Write-Output early; Start-Sleep 5",
    };

    let stopped = bash
        .run(&sandbox.ctx, json!({ "command": line, "timeout": 1000 }))
        .await
        .unwrap();

    assert!(bash.failed(&stopped), "a command stopped by its limit is a failed call");
    assert_eq!(stopped.metadata.timed_out, Some(true));
    assert!(stopped.metadata.process_id.is_none());
    assert!(stopped.output.contains("stopped after"), "{}", stopped.output);
    assert!(
        bash.spec_for(&sandbox.ctx.engine)
            .description
            .contains("are stopped after")
    );
}

#[tokio::test]
async fn with_background_processes_off_bash_offers_no_background_and_refuses_one() {
    let sandbox = Sandbox::new("bash-no-background");
    let bash = Bash::detect();
    let offered = |sandbox: &Sandbox| bash.spec_for(&sandbox.ctx.engine);
    assert!(offered(&sandbox).input_schema["properties"]["background"].is_object());

    sandbox
        .ctx
        .engine
        .store
        .set_setting(crate::background::PROCESSES_KEY, &false)
        .unwrap();
    let spec = offered(&sandbox);
    assert!(spec.input_schema["properties"]["background"].is_null());
    assert!(
        spec.description.contains("Never start long-lived servers"),
        "{}",
        spec.description
    );
    assert!(
        !sandbox.ctx.engine.shell_handoff(),
        "hand-off needs background processes"
    );

    let refused = bash
        .run(&sandbox.ctx, json!({ "command": "echo hi", "background": true }))
        .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn python_prints_as_it_goes_and_a_subagent_is_told_to_wait_for_its_process() {
    let sandbox = Sandbox::new("bash-python");
    let bash = Bash::detect();
    let show = match bash.shell {
        Shell::Bash(_) => "echo $PYTHONUNBUFFERED",
        Shell::PowerShell(_) => "Write-Output $env:PYTHONUNBUFFERED",
    };
    let shown = bash.run(&sandbox.ctx, json!({ "command": show })).await.unwrap();
    assert_eq!(shown.output.trim(), "1");

    // A subagent's process is never announced to it, so the note says to wait for it.
    let engine = &sandbox.ctx.engine;
    let workspace = engine
        .store
        .add_workspace(&sandbox.ctx.workspace.to_string_lossy(), "ws", "")
        .unwrap();
    let hidden = engine
        .store
        .create_session(crate::store::NewSession {
            workspace_id: &workspace.id,
            parent_id: None,
            visibility: crate::session::types::Visibility::Hidden,
            title: "Worker",
            agent: "general",
            model: None,
        })
        .unwrap();
    let mut worker = sandbox.ctx_clone();
    worker.session_id = hidden.id.clone();
    let sleep = match bash.shell {
        Shell::Bash(_) => "sleep 30",
        Shell::PowerShell(_) => "Start-Sleep 30",
    };

    let started = bash
        .run(&worker, json!({ "command": sleep, "background": true }))
        .await
        .unwrap();

    assert!(started.output.contains("You are a subagent"), "{}", started.output);
    engine.processes.stop_session(&hidden.id);
}
