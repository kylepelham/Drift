use std::time::Duration;

use serde_json::json;

use super::*;
use crate::llm::{Block, Credential};
use crate::session::turn::tests::{Harness, harness, prompt, text, tool_call, until_idle};
use crate::session::types::MessageWithParts;

fn ask(header: &str) -> Vec<crate::llm::Chunk> {
    let question = json!({
        "questions": [{
            "question": format!("{header}?"),
            "header": header,
            "options": [{ "label": "yes" }, { "label": "no" }],
        }],
    });
    tool_call("question", &question.to_string())
}

fn answers(transcript: &[MessageWithParts]) -> Vec<Vec<String>> {
    transcript
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|row| match &row.part {
            Part::Clarification { items, .. } => Some(items.iter().flat_map(|i| i.answers.clone()).collect()),
            _ => None,
        })
        .collect()
}

fn sent_to_model(h: &Harness) -> Vec<String> {
    h.provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .flat_map(|r| r.messages.iter().flat_map(|m| m.blocks.clone()))
        .filter_map(|b| if let Block::Text(t) = b { Some(t) } else { None })
        .collect()
}

/// A turn that asks without waiting and then ends; returns the pending request.
async fn asked(h: &Harness) -> Request {
    h.provider
        .push(ask("Deploy"))
        .push(text("asked; waiting for the answer"));
    h.engine.submit(&h.session.id, prompt("ship it")).await.unwrap();
    until_idle(h).await;
    let pending = h.engine.questions.pending();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].is_async);
    pending[0].clone()
}

#[tokio::test]
async fn an_async_question_returns_at_once_and_its_answer_starts_a_turn_once() {
    let h = harness().await;
    let request = asked(&h).await;
    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let Part::ToolCall { output, metadata, .. } = &transcript[1].parts[0].part else {
        panic!()
    };
    assert!(
        output.as_deref().unwrap().starts_with("Asked the user"),
        "the call did not wait"
    );
    assert_eq!(metadata.as_ref().unwrap().asynchronous, Some(true));

    h.provider.push(text("deploying"));
    h.engine
        .answer_question(&request.id, Some(vec![vec!["yes".into()]]))
        .await
        .unwrap();
    until_idle(&h).await;
    assert!(h.engine.questions.pending().is_empty(), "the card closed");
    assert_eq!(
        answers(&h.engine.store.transcript(&h.session.id).unwrap()),
        [vec!["yes".to_string()]]
    );
    assert!(
        sent_to_model(&h)
            .iter()
            .any(|t| t.starts_with("<question-answer") && t.contains("Deploy?\nAnswer: yes"))
    );
    assert_eq!(h.provider.responses_left(), 0, "the answer started the next turn");

    // Resent, it is the same answer and nothing new is written; a different one is refused.
    h.engine
        .answer_question(&request.id, Some(vec![vec!["yes".into()]]))
        .await
        .unwrap();
    assert_eq!(
        h.engine
            .answer_question(&request.id, Some(vec![vec!["no".into()]]))
            .await,
        Err(AnswerError::Conflict)
    );
    assert_eq!(answers(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 1);
}

#[tokio::test]
async fn the_turn_an_answer_starts_runs_at_the_sessions_reasoning_level() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.store.set_session_variant(&h.session.id, Some("max")).unwrap();
    h.provider.push(text("thought it through"));
    h.engine
        .answer_question(&request.id, Some(vec![vec!["yes".into()]]))
        .await
        .unwrap();
    until_idle(&h).await;
    let last = h.provider.requests.lock().unwrap().last().unwrap().reasoning.clone();
    assert!(
        matches!(last, Some(crate::llm::catalog::Reasoning::Budget { .. })),
        "{last:?}"
    );
}

#[tokio::test]
async fn an_answer_while_the_turn_runs_joins_it_without_starting_another() {
    let h = harness().await;
    h.provider
        .push(ask("Name"))
        .push_slow(Duration::from_millis(600), text("still working"))
        .push(text("got the name"));
    h.engine.submit(&h.session.id, prompt("work")).await.unwrap();
    let request = loop {
        if let Some(found) = h.engine.questions.pending().pop() {
            break found;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    h.engine
        .answer_question(&request.id, Some(vec![vec!["Ada".into()]]))
        .await
        .unwrap();
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0);
    assert_eq!(
        h.provider.requests.lock().unwrap().len(),
        3,
        "one turn: ask, carry on, then the answer at the next request"
    );
    assert!(sent_to_model(&h).iter().any(|t| t.contains("Answer: Ada")));
}

#[tokio::test]
async fn an_answer_after_stop_is_saved_for_the_next_turn_and_starts_nothing() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.abort(&h.session.id);
    let before = h.provider.requests.lock().unwrap().len();
    h.engine
        .answer_question(&request.id, Some(vec![vec!["no".into()]]))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !h.engine.turns.is_running(&h.session.id) && h.provider.requests.lock().unwrap().len() == before,
        "no turn after Stop"
    );
    assert_eq!(
        answers(&h.engine.store.transcript(&h.session.id).unwrap()),
        [vec!["no".to_string()]],
        "but the answer is kept"
    );

    h.provider.push(text("ok, not deploying"));
    h.engine.submit(&h.session.id, prompt("so?")).await.unwrap();
    until_idle(&h).await;
    assert!(
        sent_to_model(&h).iter().any(|t| t.contains("Answer: no")),
        "the next turn reads it"
    );
}

#[tokio::test]
async fn a_dismissed_question_closes_and_says_nothing() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.answer_question(&request.id, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.engine.questions.pending().is_empty() && !h.engine.turns.is_running(&h.session.id));
    assert!(answers(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty());
    assert_eq!(
        h.engine.answer_question(&request.id, None).await,
        Err(AnswerError::NotPending)
    );
}

#[tokio::test]
async fn an_answer_and_a_dismissal_racing_never_both_go_through() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.abort(&h.session.id);
    let yes = Some(vec![vec!["yes".into()]]);
    let (answered, dismissed) = tokio::join!(
        h.engine.answer_question(&request.id, yes),
        h.engine.answer_question(&request.id, None)
    );
    let saved = answers(&h.engine.store.transcript(&h.session.id).unwrap());
    match (answered, dismissed) {
        (Ok(()), Err(AnswerError::Conflict)) => assert_eq!(saved.len(), 1, "the answer stands"),
        (Err(AnswerError::NotPending), Ok(())) => assert!(saved.is_empty(), "the dismissal stands"),
        other => panic!("both or neither went through: {other:?}"),
    }
    assert!(h.engine.questions.pending().is_empty());
}

#[tokio::test]
async fn identical_answers_racing_save_once_and_both_succeed() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.abort(&h.session.id);
    let yes = || Some(vec![vec!["yes".to_string()]]);
    let (first, second) = tokio::join!(
        h.engine.answer_question(&request.id, yes()),
        h.engine.answer_question(&request.id, yes())
    );
    assert_eq!((first, second), (Ok(()), Ok(())));
    assert_eq!(answers(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 1);
}

#[tokio::test]
async fn a_saved_answer_is_recognised_from_the_store_once_its_card_is_gone() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.abort(&h.session.id);
    h.engine
        .answer_question(&request.id, Some(vec![vec!["no".into()]]))
        .await
        .unwrap();
    // As after a restart: nothing in memory remembers the question.
    h.engine.questions.forget_session(&h.session.id);
    h.engine
        .answer_question(&request.id, Some(vec![vec!["no".into()]]))
        .await
        .unwrap();
    assert_eq!(
        h.engine
            .answer_question(&request.id, Some(vec![vec!["yes".into()]]))
            .await,
        Err(AnswerError::Conflict)
    );
    assert_eq!(
        h.engine.answer_question(&request.id, None).await,
        Err(AnswerError::Conflict),
        "a saved answer cannot be dismissed"
    );
    assert_eq!(answers(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 1);
}

#[tokio::test]
async fn an_answer_that_cannot_be_saved_keeps_its_card() {
    let h = harness().await;
    let request = asked(&h).await;
    h.engine.credentials.remove("anthropic").unwrap();
    let failed = h
        .engine
        .answer_question(&request.id, Some(vec![vec!["yes".into()]]))
        .await;
    assert_eq!(failed, Err(AnswerError::Turn(TurnError::NoCredentials)));
    assert_eq!(h.engine.questions.pending().len(), 1, "still answerable");
    assert!(answers(&h.engine.store.transcript(&h.session.id).unwrap()).is_empty());

    h.engine
        .credentials
        .set("anthropic", &Credential::ApiKey { key: "k".into() })
        .unwrap();
    h.provider.push(text("thanks"));
    h.engine
        .answer_question(&request.id, Some(vec![vec!["yes".into()]]))
        .await
        .unwrap();
    until_idle(&h).await;
    assert_eq!(answers(&h.engine.store.transcript(&h.session.id).unwrap()).len(), 1);
}

#[tokio::test]
async fn a_subagent_always_waits_for_its_answer() {
    let h = harness().await;
    h.provider
        .push_for(
            "PARENT",
            tool_call("task", r#"{"description": "Ask", "prompt": "CHILD ask"}"#),
        )
        .push_for("CHILD ask", ask("Colour"))
        .push_for("CHILD ask", text("blue it is"))
        .push_for("PARENT", text("done"));
    h.engine.submit(&h.session.id, prompt("PARENT go")).await.unwrap();
    let request = loop {
        if let Some(found) = h.engine.questions.pending().pop() {
            break found;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        !request.is_async,
        "a worker's turn would end before a late answer could reach anyone"
    );
    h.engine
        .answer_question(&request.id, Some(vec![vec!["blue".into()]]))
        .await
        .unwrap();
    until_idle(&h).await;
    assert_eq!(h.provider.responses_left(), 0);
}
