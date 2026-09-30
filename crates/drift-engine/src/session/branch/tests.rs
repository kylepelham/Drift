use std::time::Duration;

use serde_json::json;

use super::*;
use crate::llm::Block;
use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle, Harness};
use crate::session::types::Role;

const HANDOFF: &str = "TITLE: Fix the lint\nSUMMARY:\nWe tidied the parser in src/parse.rs.\nEXCERPTS:\nerror: unused import";

async fn conversation(h: &Harness) {
    h.provider.push(text("Parser tidied"));
    h.engine.submit(&h.session.id, prompt("tidy the parser")).await.unwrap();
    until_idle(h).await;
}

async fn until_session_idle(h: &Harness, id: &str) {
    for _ in 0..300 {
        if !h.engine.turns.is_running(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} never finished");
}

fn draft(goal: &str, cutoff: Option<String>) -> BranchDraft {
    BranchDraft { goal: goal.into(), title: "Fix the lint".into(), summary: "We tidied the parser.".into(), excerpts: String::new(), cutoff }
}

#[tokio::test]
async fn a_draft_summarises_the_source_without_changing_it() {
    let h = harness().await;
    conversation(&h).await;
    let before = h.engine.store.transcript(&h.session.id).unwrap();
    h.provider.push(text(HANDOFF));
    let draft = h.engine.draft_branch(&h.session.id, "fix the lint errors").await.unwrap();
    assert_eq!(draft.title, "Fix the lint");
    assert_eq!(draft.summary, "We tidied the parser in src/parse.rs.");
    assert_eq!(draft.excerpts, "error: unused import");
    assert_eq!(draft.cutoff.as_deref(), Some(before[1].info.id.as_str()));
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().len(), before.len(), "drafting stores nothing");

    let requests = h.provider.requests.lock().unwrap().clone();
    let request = requests.last().unwrap();
    assert_eq!(request.messages.len(), 3, "source history plus the handoff instruction");
    let Some(Block::Text(instruction)) = request.messages[2].blocks.last() else { panic!() };
    assert!(instruction.contains("fix the lint errors") && instruction.contains("do not call tools"));
}

#[tokio::test]
async fn a_branch_records_its_source_and_cutoff_and_starts_with_the_handoff() {
    let h = harness().await;
    conversation(&h).await;
    let cutoff = h.engine.store.transcript(&h.session.id).unwrap()[1].info.id.clone();
    h.provider.push(text("Lint fixed"));
    let branch = h.engine.branch(&h.session.id, draft("fix the lint errors", Some(cutoff.clone()))).await.unwrap();
    until_session_idle(&h, &branch.id).await;
    let stored = h.engine.store.session(&branch.id).unwrap().unwrap();
    assert_eq!(stored.parent_id.as_deref(), Some(h.session.id.as_str()));
    assert_eq!(stored.visibility, Visibility::Sibling);
    assert_eq!(stored.branch_cutoff.as_deref(), Some(cutoff.as_str()));
    assert_eq!(stored.title, "Fix the lint");
    let transcript = h.engine.store.transcript(&branch.id).unwrap();
    assert_eq!(transcript[0].info.role, Role::User);
    let Part::Text { text } = &transcript[0].parts[0].part else { panic!() };
    assert!(text.contains("We tidied the parser.") && text.ends_with("# Goal\n\nfix the lint errors"), "{text}");
}

#[tokio::test]
async fn stopping_the_source_does_not_stop_its_branch() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy { rules: vec![Rule { kind: "bash".into(), pattern: "*".into(), decision: Decision::Allow }] });
    let sleep = if cfg!(windows) { "ping -n 10 127.0.0.1 > nul" } else { "sleep 10" };
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    h.engine.submit(&h.session.id, prompt("wait")).await.unwrap();
    h.provider.push(tool_call("bash", &json!({ "command": sleep }).to_string()));
    let branch = h.engine.branch(&h.session.id, draft("wait elsewhere", None)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    assert!(h.engine.turns.is_running(&branch.id), "a branch is not a worker of its source");
    assert!(h.engine.abort(&branch.id));
    until_session_idle(&h, &branch.id).await;
}

#[tokio::test]
async fn subagents_empty_goals_and_foreign_cutoffs_are_refused() {
    let h = harness().await;
    conversation(&h).await;
    let subagent = h
        .engine
        .store
        .create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: Some(&h.session.id), visibility: Visibility::Hidden, title: "", agent: "build", model: None })
        .unwrap();
    assert!(matches!(h.engine.draft_branch(&subagent.id, "anything").await, Err(BranchError::FromSubagent)));
    assert!(matches!(h.engine.branch(&subagent.id, draft("anything", None)).await, Err(BranchError::FromSubagent)));
    assert!(matches!(h.engine.draft_branch(&h.session.id, "  ").await, Err(BranchError::EmptyGoal)));
    let other = h
        .engine
        .store
        .create_session(NewSession { workspace_id: &h.session.workspace_id, parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None })
        .unwrap();
    let foreign = h.engine.store.create_message(&other.id, Role::User, None).unwrap();
    assert!(matches!(h.engine.branch(&h.session.id, draft("x", Some(foreign.id))).await, Err(BranchError::BadCutoff)));
    let count: i64 = h.engine.store.lock().query_row("SELECT COUNT(*) FROM session WHERE branch_cutoff IS NOT NULL OR title = 'Fix the lint'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0, "refused branches leave nothing behind");
}

#[test]
fn unlabelled_handoffs_become_the_summary() {
    let parsed = parse_draft("fix the flaky test in the scheduler suite please", "Just some prose.", None);
    assert_eq!(parsed.summary, "Just some prose.");
    assert_eq!(parsed.title, "fix the flaky test in the");
    let none = parse_draft("goal", "TITLE: T\nSUMMARY:\nS\nEXCERPTS:\nnone", None);
    assert_eq!((none.title.as_str(), none.summary.as_str(), none.excerpts.as_str()), ("T", "S", ""));
}
