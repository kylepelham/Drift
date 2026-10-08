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
