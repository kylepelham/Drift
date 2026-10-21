use std::process::Stdio;
use std::sync::Mutex;

use super::buffer::{Buffer, READ_BYTES, url_in};
use super::*;

fn log() -> PathBuf {
    crate::platform::temp::test_run()
        .join(format!("drift-background-{}", crate::random_hex(4)))
        .join("p.log")
}

#[test]
fn each_read_hands_over_only_what_is_new_and_the_log_keeps_everything() {
    let mut buffer = Buffer::new(log());
    buffer.seen(b"shown already\n");
    buffer.push(b"one\ntwo\n");

    let first = buffer.unread();
    assert_eq!(first.text, "one\ntwo\n");
    assert_eq!(first.skipped, 0);
    assert_eq!(buffer.unread().text, "", "nothing new");

    buffer.push(&vec![b'x'; READ_BYTES + 10]);
    let big = buffer.unread();
    assert_eq!(big.text.len(), READ_BYTES);
    assert_eq!(big.skipped, 10, "older new output is left to the log");

    let logged = std::fs::read(&buffer.path).unwrap();
    assert_eq!(logged.len() as u64, buffer.total());
    std::fs::remove_dir_all(buffer.path.parent().unwrap()).ok();
}

#[test]
fn searches_and_waits_look_at_lines() {
    let mut buffer = Buffer::new(log());
    buffer.push(b"compiling\nerror: one\nok\nerror: two\n");
    let errors = |line: &str| line.starts_with("error");

    assert_eq!(buffer.search(&errors, 1), ["error: two"]);
    assert!(buffer.unread_matches(&errors));
    buffer.unread();
    assert!(!buffer.unread_matches(&errors), "read output no longer counts");
    assert_eq!(buffer.last_line().as_deref(), Some("error: two"));
    std::fs::remove_dir_all(buffer.path.parent().unwrap()).ok();
}

#[test]
fn a_dev_servers_address_is_found_in_its_line() {
    assert_eq!(
        url_in("  ➜  Local:   http://localhost:5173/").as_deref(),
        Some("http://localhost:5173/")
    );
    assert_eq!(
        url_in("Listening on https://127.0.0.1:8443.").as_deref(),
        Some("https://127.0.0.1:8443")
    );
    assert_eq!(url_in("no address here"), None);
    assert_eq!(url_in("fetched http://example.com:8080/x"), None, "not this machine");
    assert_eq!(
        url_in("refused http://127.0.0.1:1 as expected"),
        None,
        "not a server's port"
    );
    assert_eq!(
        url_in("see https://github.com then http://localhost:3000").as_deref(),
        Some("http://localhost:3000")
    );
}

/// A shell line run as a background process, with what its watcher heard.
fn started(processes: &Processes, line: &str) -> (Arc<Handle>, Arc<Mutex<Vec<ProcessRecord>>>) {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/C", line]);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", line]);
        command
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::platform::process::prepare(&mut command);
    let mut child = command.spawn().unwrap();
    let tree = child.id().and_then(|pid| Tree::adopt(pid).ok());
    let pipes = Pipes::take(&mut child);

    let heard = Arc::new(Mutex::new(Vec::new()));
    let recording = heard.clone();
    let watcher = Watcher {
        changed: Box::new(move |record| recording.lock().unwrap().push(record)),
        ended: Box::new(|_| {}),
    };
    let start = Start {
        session_id: "s".into(),
        command: line.into(),
        description: "test".into(),
        child,
        tree,
        pipes,
        seen: Vec::new(),
        started_at: crate::id::now_ms(),
    };

    (
        processes
            .start(start, 0, log().parent().unwrap().into(), watcher)
            .handle()
            .clone(),
        heard,
    )
}

#[tokio::test]
async fn a_process_that_exits_is_recorded_with_its_code_and_output() {
    let processes = Processes::default();
    let (handle, heard) = started(&processes, "echo ready && exit 3");

    let waited = handle
        .wait(None, Duration::from_secs(10), &CancellationToken::new())
        .await;

    assert_eq!(waited, Waited::Ended);
    assert_eq!(handle.exit(), Some(Exit::Code(3)));
    assert!(handle.unread().text.contains("ready"));
    let last = heard.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.state, ProcessState::Exited);
    assert_eq!(last.exit_code, Some(3));
    assert!(handle.claim_report() && !handle.claim_report(), "its end is told once");
}

#[tokio::test]
async fn a_wait_ends_at_the_line_it_waits_for_and_stop_ends_the_process() {
    let processes = Processes::default();
    let line = if cfg!(windows) {
        "echo booting && echo listening on http://localhost:4000 && ping -n 30 127.0.0.1 > nul"
    } else {
        "echo booting; echo listening on http://localhost:4000; sleep 30"
    };
    let (handle, _) = started(&processes, line);
    let listening = |line: &str| line.contains("listening");

    let waited = handle
        .wait(Some(&listening), Duration::from_secs(10), &CancellationToken::new())
        .await;
    assert_eq!(waited, Waited::Matched);
    assert_eq!(handle.record().url.as_deref(), Some("http://localhost:4000"));
    assert_eq!(processes.of("s").len(), 1);

    processes.stop_session("s");
    let ended = handle
        .wait(None, Duration::from_secs(10), &CancellationToken::new())
        .await;
    assert_eq!(ended, Waited::Ended);
    assert_eq!(handle.record().state, ProcessState::Stopped);
}

#[tokio::test]
async fn input_reaches_a_process_reading_it() {
    let processes = Processes::default();
    let line = if cfg!(windows) {
        "set /p name= && call echo hello %^name%"
    } else {
        "read name; echo hello $name"
    };
    let (handle, _) = started(&processes, line);

    assert!(handle.input("drift\n").await);
    handle
        .wait(None, Duration::from_secs(10), &CancellationToken::new())
        .await;

    assert!(handle.unread().text.contains("hello drift"), "{:?}", handle.tail(200));
}

#[tokio::test]
async fn an_exit_is_told_only_after_the_calls_watching_it_are_done() {
    let processes = Processes::default();
    let (handle, _) = started(&processes, "echo done");
    let watching = handle.watch();
    handle
        .wait(None, Duration::from_secs(10), &CancellationToken::new())
        .await;
    assert!(handle.exit().is_some());

    // What tells the conversation waits while a call that may say it ended is still writing its result.
    let told = tokio::spawn({
        let handle = handle.clone();
        async move { handle.unwatched().await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!told.is_finished(), "a call is still watching");

    assert!(watching.claim_report(), "the watching call says it ended");
    drop(watching);
    tokio::time::timeout(Duration::from_secs(5), told)
        .await
        .unwrap()
        .unwrap();
    assert!(!handle.claim_report(), "so it is not told again");
}
