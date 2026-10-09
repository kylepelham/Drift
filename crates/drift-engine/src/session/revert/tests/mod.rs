use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::TurnError;
use crate::session::turn::tests::{Harness, harness, prompt, text, tool_call, until_idle};
use crate::session::types::MessageStatus;
use serde_json::json;
use std::time::Duration;

use super::*;

mod history;
mod lifecycle;
mod maintenance;
mod rollback;

fn allow_writes(h: &Harness) {
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "edit".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
    });
}

fn allow_shell(h: &Harness) {
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "bash".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
    });
}

fn write(path: &str, content: &str) -> Vec<crate::llm::Chunk> {
    tool_call("write", &json!({ "path": path, "content": content }).to_string())
}

async fn turn(h: &Harness, ask: &str) {
    h.engine.submit(&h.session.id, prompt(ask)).await.unwrap();
    until_idle(h).await;
}

fn read(h: &Harness, path: &str) -> Option<String> {
    std::fs::read_to_string(h._dir.join("ws").join(path)).ok()
}

/// Two turns: the first writes a.txt, the second rewrites it and adds b.txt. Returns both prompts' ids.
async fn two_writing_turns(h: &Harness) -> (String, String) {
    allow_writes(h);
    h.provider.push(write("a.txt", "one")).push(text("wrote a"));
    turn(h, "first").await;
    h.provider
        .push(write("a.txt", "two"))
        .push(write("b.txt", "bee"))
        .push(text("rewrote"));
    turn(h, "second").await;
    let prompts: Vec<_> = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .filter(|message| message.info.role == Role::User)
        .map(|message| message.info.id.clone())
        .collect();

    (prompts[0].clone(), prompts[1].clone())
}

/// Makes every save of the session's undo point fail until `allow_marker` is called.
fn refuse_marker(h: &Harness) {
    let trigger = "CREATE TRIGGER refuse_marker BEFORE UPDATE OF revert_json ON session \
                   BEGIN SELECT RAISE(FAIL, 'injected'); END;";
    h.engine.store.lock().execute_batch(trigger).unwrap();
}

fn allow_marker(h: &Harness) {
    h.engine
        .store
        .lock()
        .execute_batch("DROP TRIGGER refuse_marker;")
        .unwrap();
}

async fn overlapping_writes(break_chain: bool) -> (Harness, String, String, PathBuf) {
    let h = harness().await;
    allow_writes(&h);
    let nested = h._dir.join("ws/sub");
    std::fs::create_dir_all(&nested).unwrap();
    let file = nested.join("a.txt");
    std::fs::write(&file, "A").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path":"sub/a.txt"}"#))
        .push(text("read"));
    turn(&h, "read the file").await;
    h.provider.push(write("sub/a.txt", "B")).push(text("first write"));
    let first = h
        .engine
        .submit(&h.session.id, prompt("first write"))
        .await
        .unwrap()
        .message
        .id;
    until_idle(&h).await;

    let workspace = h
        .engine
        .store
        .add_workspace(&nested.to_string_lossy(), "nested", "")
        .unwrap();
    h.engine.move_session(&h.session.id, &workspace.id).unwrap();
    if break_chain {
        std::fs::write(&file, "X").unwrap();
    }
    h.provider.push(write("a.txt", "C")).push(text("second write"));
    let second = h
        .engine
        .submit(&h.session.id, prompt("second write"))
        .await
        .unwrap()
        .message
        .id;
    until_idle(&h).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "C");

    (h, first, second, file)
}

/// Waits until a tool call is running: its before-state has been captured and it has started.
async fn until_running_call(h: &Harness) {
    for _ in 0..400 {
        let transcript = h.engine.store.transcript(&h.session.id).unwrap();
        let running = transcript.iter().flat_map(|message| &message.parts).any(|row| {
            matches!(
                row.part,
                Part::ToolCall {
                    status: crate::session::types::ToolStatus::Running,
                    ..
                }
            )
        });
        if running {
            return;
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no call started");
}
