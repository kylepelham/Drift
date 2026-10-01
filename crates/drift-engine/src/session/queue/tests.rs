use std::time::Duration;

use crate::llm::Provider;
use crate::session::turn::tests::{harness, prompt, text, tool_call, Harness};
use crate::session::turn::{Prompt, TurnError};
use crate::session::types::{Part, Role};
use crate::Engine;

/// Idle with nothing waiting: a handover leaves a gap where neither holds, so idle alone is not enough.
async fn until_settled(engine: &Engine, session_id: &str) {
    for _ in 0..1000 {
        if !engine.turns.is_running(session_id) && engine.store.queued(session_id).unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never settled");
}

fn as_agent(agent: &str, words: &str) -> Prompt {
    Prompt { agent: Some(agent.into()), ..prompt(words) }
}

/// A build turn that takes a while over its first step and then answers.
async fn busy(h: &Harness) {
    h.provider.push_slow(Duration::from_millis(400), tool_call("read", r#"{"path": "missing.txt"}"#)).push(text("built"));
    h.engine.submit(&h.session.id, prompt("build it")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

fn texts(parts: &[Part]) -> Vec<&str> {
    parts.iter().filter_map(|part| if let Part::Text { text } = part { Some(text.as_str()) } else { None }).collect()
}

#[tokio::test]
async fn a_prompt_for_another_agent_waits_without_holding_the_caller_then_runs_as_that_agent() {
    let h = harness().await;
    busy(&h).await;
    h.provider.push(text("planned"));
    let receipt = tokio::time::timeout(Duration::from_millis(100), h.engine.submit(&h.session.id, as_agent("plan", "plan instead"))).await.expect("answers at once").unwrap();
    assert!(receipt.message.is_none(), "nothing admitted yet");
    let queued = receipt.session.queued.expect("the session shows what waits");
    assert_eq!((queued.agent.as_str(), queued.text.as_str()), ("plan", "plan instead"));
    until_settled(&h.engine, &h.session.id).await;
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "the build turn ends after its step instead of answering the plan prompt");
    assert!(!format!("{:?}", requests[0].messages).contains("plan instead"));
    assert!(format!("{:?}", requests[1].messages).contains("plan instead"));
    assert!(!requests[1].tools.iter().any(|t| t.name == "write" || t.name == "edit"), "answered with plan's tools");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let reply = transcript.last().unwrap();
    assert_eq!((reply.info.role, reply.info.agent.as_deref()), (Role::Assistant, Some("plan")));
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().queued, None);
}

#[tokio::test]
async fn a_model_change_sent_mid_turn_waits_and_runs_on_that_model() {
    let h = harness().await;
    busy(&h).await;
    h.provider.push(text("on the other model"));
    let other = h.engine.catalog.read().unwrap().providers["anthropic"].models.keys().find(|id| *id != "claude-sonnet-4-5").unwrap().clone();
    let other = crate::session::types::ModelRef { provider: "anthropic".into(), model: other };
    let receipt = h.engine.submit(&h.session.id, Prompt { model: Some(other.clone()), ..prompt("try opus") }).await.unwrap();
    assert!(receipt.message.is_none(), "it does not join a turn on the old model");
    assert_eq!(receipt.session.queued.unwrap().model.as_ref(), Some(&other), "the composer can show what it waits on");
    let joined = h.engine.submit(&h.session.id, Prompt { model: Some(other.clone()), ..prompt("and then") }).await.unwrap();
    assert!(joined.returned.is_empty(), "a follow-up on the waiting model joins it");
    until_settled(&h.engine, &h.session.id).await;
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.last().unwrap().model, other.model);
    assert!(format!("{:?}", requests.last().unwrap().messages).contains("and then"));
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().model, Some(other));
}

#[tokio::test]
async fn a_waiting_submission_sent_again_is_one_prompt_and_its_id_cannot_carry_another() {
    let h = harness().await;
    busy(&h).await;
    h.provider.push(text("planned"));
    let once = Prompt { submission_id: Some("sub_wait".into()), ..as_agent("plan", "plan it") };
    h.engine.submit(&h.session.id, once.clone()).await.unwrap();
    let again = h.engine.submit(&h.session.id, once.clone()).await.unwrap();
    assert!(again.message.is_none() && again.returned.is_empty());
    assert_eq!(h.engine.store.queued(&h.session.id).unwrap().len(), 1, "queued once");
    let other = Prompt { submission_id: Some("sub_wait".into()), ..as_agent("plan", "something else") };
    assert_eq!(h.engine.submit(&h.session.id, other).await.err(), Some(TurnError::SubmissionReused));
    until_settled(&h.engine, &h.session.id).await;
    let landed = h.engine.submit(&h.session.id, once).await.unwrap();
    assert!(landed.message.is_some(), "after it ran, the same id replays the message it became");
    let users = h.engine.store.transcript(&h.session.id).unwrap().iter().filter(|m| m.info.role == Role::User).count();
    assert_eq!(users, 2);
}

#[tokio::test]
async fn prompts_sent_while_one_waits_join_it_or_replace_it() {
    let h = harness().await;
    busy(&h).await;
    h.provider.push(text("reviewed"));
    h.engine.submit(&h.session.id, as_agent("plan", "plan it")).await.unwrap();
    let joined = h.engine.submit(&h.session.id, prompt("and the tests")).await.unwrap();
    assert_eq!(joined.session.queued.unwrap().text, "plan it\n\nand the tests", "naming no agent, it runs with the one waiting");
    let replaced = h.engine.submit(&h.session.id, as_agent("build", "just build")).await.unwrap();
    assert_eq!(texts(&replaced.returned), ["plan it", "and the tests"], "given back, never run");
    until_settled(&h.engine, &h.session.id).await;
    let requests = h.provider.requests.lock().unwrap();
    let last = format!("{:?}", requests.last().unwrap().messages);
    assert!(last.contains("just build") && !last.contains("plan it"));
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.last().unwrap().info.agent.as_deref(), Some("build"));
}

#[tokio::test]
async fn discarding_lets_the_turn_carry_on_and_stop_never_starts_what_waited() {
    let h = harness().await;
    busy(&h).await;
    h.engine.submit(&h.session.id, as_agent("plan", "plan it")).await.unwrap();
    assert_eq!(texts(&h.engine.discard_queued(&h.session.id)), ["plan it"]);
    until_settled(&h.engine, &h.session.id).await;
    assert_eq!(h.provider.requests.lock().unwrap().len(), 2, "the build turn answered its own step");

    h.provider.push_slow(Duration::from_millis(400), tool_call("read", r#"{"path": "missing.txt"}"#));
    h.engine.submit(&h.session.id, prompt("again")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.engine.submit(&h.session.id, as_agent("plan", "plan after")).await.unwrap();
    assert!(h.engine.abort(&h.session.id));
    until_settled(&h.engine, &h.session.id).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!h.engine.turns.is_running(&h.session.id), "nothing started after the Stop");
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(!format!("{transcript:?}").contains("plan after"));
}

fn waiting_row(prompt: &Prompt) -> crate::store::QueuedRow {
    let id = prompt.submission_id.clone().unwrap();
    crate::store::QueuedRow { submission_id: id, payload_hash: crate::session::turn::payload_hash(prompt), prompt_json: serde_json::to_string(prompt).unwrap(), error: None, created_at: 0 }
}

#[tokio::test]
async fn a_start_planned_before_the_prompt_was_taken_back_writes_nothing_and_its_replacement_runs() {
    let h = harness().await;
    let given_back = Prompt { submission_id: Some("sub_back".into()), ..as_agent("plan", "given back") };
    let replacement = Prompt { submission_id: Some("sub_new".into()), ..as_agent("build", "instead") };
    // The start read these rows; then a replacement took them back before it wrote anything.
    let hash = crate::session::turn::payload_hash(&given_back);
    let stale = [("sub_back", hash.as_str())];
    h.engine.store.queue(&h.session.id, &waiting_row(&replacement), false).unwrap();
    let started = h.engine.admit(&h.session.id, given_back, crate::session::turn::Admission { queued: &stale, ..Default::default() }).await;
    assert_eq!(started.err(), Some(TurnError::SubmissionReused));
    assert!(h.engine.store.transcript(&h.session.id).unwrap().is_empty(), "the prompt handed back never ran");
    assert!(!h.engine.turns.is_running(&h.session.id), "its claim was let go");
    h.provider.push(text("built"));
    h.engine.start_queued(&h.session.id).await;
    until_settled(&h.engine, &h.session.id).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(format!("{transcript:?}").contains("instead") && !format!("{transcript:?}").contains("given back"));
}

#[tokio::test]
async fn a_turn_the_engine_starts_while_a_prompt_waits_still_answers_before_handing_over() {
    let h = harness().await;
    let waiting = Prompt { submission_id: Some("sub_plan".into()), ..as_agent("plan", "plan next") };
    h.engine.store.queue(&h.session.id, &waiting_row(&waiting), false).unwrap();
    h.provider.push(text("noted your answer")).push(text("planned"));
    let answer = Part::Clarification { request_id: "q1".into(), items: vec![] };
    h.engine.submit(&h.session.id, Prompt { parts: vec![answer], ..prompt("") }).await.unwrap();
    until_settled(&h.engine, &h.session.id).await;
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "the answer got a reply of its own, then the waiting prompt ran");
    assert!(!format!("{:?}", requests[0].messages).contains("plan next"));
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let roles: Vec<_> = transcript.iter().map(|m| (m.info.role, m.info.agent.clone().unwrap_or_default())).collect();
    assert_eq!(roles, [(Role::User, "build".into()), (Role::Assistant, "build".into()), (Role::User, "plan".into()), (Role::Assistant, "plan".into())]);
}

#[tokio::test]
async fn an_old_start_failing_after_its_replacement_leaves_the_replacement_to_run() {
    let h = harness().await;
    let old = Prompt { submission_id: Some("sub_old".into()), ..as_agent("plan", "old goal") };
    let new = Prompt { submission_id: Some("sub_new".into()), ..as_agent("build", "new goal") };
    h.engine.store.queue(&h.session.id, &waiting_row(&old), false).unwrap();
    let old_hash = crate::session::turn::payload_hash(&old);
    // The old start was planning (a credential refresh, say) when the user replaced it.
    h.engine.store.queue(&h.session.id, &waiting_row(&new), true).unwrap();
    h.provider.push(text("built"));
    h.engine.fail_queue(&h.session.id, &[("sub_old", old_hash.as_str())], "provider has no credentials");
    until_settled(&h.engine, &h.session.id).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    assert!(format!("{transcript:?}").contains("new goal"), "the replacement ran instead of taking the old failure");
    assert_eq!(h.engine.store.session(&h.session.id).unwrap().unwrap().queued, None);
}

#[tokio::test]
async fn what_waited_starts_after_a_restart() {
    let h = harness().await;
    let waiting = Prompt { submission_id: Some("sub_left".into()), ..as_agent("plan", "left waiting") };
    let row = crate::store::QueuedRow { submission_id: "sub_left".into(), payload_hash: crate::session::turn::payload_hash(&waiting), prompt_json: serde_json::to_string(&waiting).unwrap(), error: None, created_at: 0 };
    h.engine.store.queue(&h.session.id, &row, false).unwrap();

    let reopened = Engine::open_with(&h._dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
    *reopened.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(h.provider.clone()));
    h.provider.push(text("planned"));
    reopened.resume_queued();
    until_settled(&reopened, &h.session.id).await;
    let transcript = reopened.store.transcript(&h.session.id).unwrap();
    assert_eq!(transcript.len(), 2);
    assert_eq!(transcript[1].info.agent.as_deref(), Some("plan"));
    assert!(reopened.store.submission("sub_left").unwrap().is_some(), "landed under its own id");
}

#[tokio::test]
async fn one_that_cannot_start_stays_with_its_reason_and_holds_nothing() {
    let h = harness().await;
    busy(&h).await;
    let mut doomed = as_agent("plan", "on a model that is gone");
    doomed.model = Some(crate::session::types::ModelRef { provider: "anthropic".into(), model: "no-such-model".into() });
    h.engine.submit(&h.session.id, doomed).await.unwrap();
    for _ in 0..200 {
        if h.engine.store.session(&h.session.id).unwrap().unwrap().queued.is_some_and(|q| q.error.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let queued = h.engine.store.session(&h.session.id).unwrap().unwrap().queued.unwrap();
    assert_eq!(queued.error.as_deref(), Some(TurnError::UnknownModel.to_string().as_str()));
    assert!(!h.engine.turns.is_running(&h.session.id));
    assert_eq!(texts(&h.engine.discard_queued(&h.session.id)), ["on a model that is gone"]);
}
