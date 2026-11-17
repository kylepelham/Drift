use super::*;

#[tokio::test]
async fn aborting_the_shell_stops_its_descendants() {
    let sandbox = Sandbox::new("bash-tree");
    let bash = Bash::detect();
    let command = match bash.shell {
        Shell::Bash(_) => "(sleep 2; echo late > late.txt) & sleep 30",
        Shell::PowerShell(_) => {
            "Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep 2; Set-Content late.txt late'; Start-Sleep 30"
        }
    };
    let context = sandbox.ctx_clone();
    let abort = context.abort.clone();
    let running = tokio::spawn(async move { Bash::detect().run(&context, json!({ "command": command })).await });
    tokio::time::sleep(Duration::from_millis(600)).await;

    abort.cancel();
    let result = running.await.unwrap().unwrap();
    assert_eq!(result.metadata.stopped, Some(true));
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(
        !sandbox.ctx.workspace.join("late.txt").exists(),
        "a descendant kept running after Stop"
    );
}

#[tokio::test]
async fn dropping_the_run_future_also_stops_descendants() {
    let sandbox = Sandbox::new("bash-drop");
    let bash = Bash::detect();
    let command = match bash.shell {
        Shell::Bash(_) => "(sleep 2; echo late > dropped.txt) & sleep 30",
        Shell::PowerShell(_) => {
            "Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep 2; Set-Content dropped.txt late'; Start-Sleep 30"
        }
    };
    let context = sandbox.ctx_clone();
    let handle = tokio::spawn(async move { Bash::detect().run(&context, json!({ "command": command })).await });
    tokio::time::sleep(Duration::from_millis(600)).await;

    handle.abort();
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(
        !sandbox.ctx.workspace.join("dropped.txt").exists(),
        "a descendant survived the future being dropped"
    );
}

#[tokio::test]
async fn a_detached_command_outlives_its_call_and_stop() {
    let sandbox = Sandbox::new("bash-detach");
    let bash = Bash::detect();
    let command = match bash.shell {
        Shell::Bash(_) => "(sleep 2; echo late > detached.txt) & echo launched",
        Shell::PowerShell(_) => {
            "Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep 2; Set-Content detached.txt late'; 'launched'"
        }
    };
    let result = bash
        .run(&sandbox.ctx, json!({ "command": command, "detach": true }))
        .await
        .unwrap();
    assert!(result.output.contains("launched"), "{}", result.output);
    assert!(result.output.contains("keeps running"), "{}", result.output);

    sandbox.ctx.abort.cancel();
    tokio::time::sleep(Duration::from_millis(4000)).await;
    assert!(
        sandbox.ctx.workspace.join("detached.txt").exists(),
        "what a detached command started was stopped with its call"
    );
}

#[tokio::test]
async fn a_detached_launch_that_fails_says_so() {
    let sandbox = Sandbox::new("bash-detach-fail");
    let result = Bash::detect()
        .run(&sandbox.ctx, json!({ "command": "exit 3", "detach": true }))
        .await
        .unwrap();
    assert!(result.output.contains("exit code 3"), "{}", result.output);
}
