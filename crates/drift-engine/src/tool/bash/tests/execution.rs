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
async fn a_workdir_inside_the_workspace_is_where_it_runs_and_one_outside_is_refused() {
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
    let outside = bash
        .run(&sandbox.ctx, json!({ "command": print, "workdir": ".." }))
        .await
        .unwrap_err();
    assert!(outside.0.contains("outside the workspace"), "{}", outside.0);

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

    collect(&mut child, &mut spool, &progress).await;
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
async fn times_out_and_aborts() {
    let sandbox = Sandbox::new("bash-timeout");
    let bash = Bash::detect();
    let sleep = match bash.shell {
        Shell::Bash(_) => "sleep 5",
        Shell::PowerShell(_) => "Start-Sleep 5",
    };
    let early_then_sleep = match bash.shell {
        Shell::Bash(_) => "echo early; sleep 5",
        Shell::PowerShell(_) => "Write-Output early; Start-Sleep 5",
    };

    let stopped = bash
        .run(&sandbox.ctx, json!({ "command": early_then_sleep, "timeout": 1500 }))
        .await
        .unwrap();
    assert!(bash.failed(&stopped), "a command stopped by its limit is a failed call");
    assert_eq!(
        (stopped.metadata.timed_out, stopped.metadata.shell_timeout_ms.flatten()),
        (Some(true), Some(1500))
    );
    assert!(stopped.output.contains("stopped after"), "{}", stopped.output);
    assert!(
        stopped.output.starts_with("early"),
        "what it printed before the limit is kept: {}",
        stopped.output
    );

    sandbox.ctx.engine.set_shell_timeout(Some(Duration::from_millis(300)));
    assert_eq!(
        bash.running_metadata(&sandbox.ctx, &json!({ "command": sleep }))
            .unwrap()
            .shell_timeout_ms,
        Some(Some(300))
    );
    let by_setting = bash.run(&sandbox.ctx, json!({ "command": sleep })).await.unwrap();
    assert_eq!(
        by_setting.metadata.timed_out,
        Some(true),
        "without a `timeout` the Settings limit applies"
    );

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
