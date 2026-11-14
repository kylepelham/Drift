use std::time::Duration;

use crate::session::tasks::{Mode, TaskState};
use crate::session::turn::tests::{Harness, harness, model, prompt, text, tool, until_idle};
use crate::session::types::{MessageWithParts, Role};

use super::*;

fn select_model(h: &Harness) {
    h.engine
        .store
        .update_session(&h.session.id, None, Some(&model()), None)
        .unwrap();
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..600 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

fn transcript(h: &Harness) -> Vec<MessageWithParts> {
    h.engine.store.transcript(&h.session.id).unwrap()
}

fn sent(h: &Harness, index: usize) -> String {
    format!("{:?}", h.provider.requests.lock().unwrap()[index].messages)
}

fn opens_with(h: &Harness, index: usize, wanted: &str) -> bool {
    let requests = h.provider.requests.lock().unwrap();
    let first = requests[index]
        .messages
        .first()
        .and_then(|message| message.blocks.first());

    matches!(first, Some(crate::llm::Block::Text(first)) if first == wanted)
}

fn job(arguments: &str) -> Result<Job, String> {
    parse(arguments, &Catalog::bundled(), Some("anthropic"))
}

#[test]
fn the_words_before_the_prompt_choose_when_and_on_which_model_it_runs() {
    let named = job("seq anthropic/claude-haiku-4-5 look at\nthe parser").unwrap();
    assert_eq!(named.mode, Mode::Foreground);
    assert_eq!(named.model.as_ref().unwrap().model, "claude-haiku-4-5");
    assert_eq!(named.prompt, "look at\nthe parser", "the prompt keeps its own lines");

    let bare = job("claude-haiku-4-5 look").unwrap();
    assert_eq!(
        (bare.mode, bare.model.unwrap().provider.as_str()),
        (Mode::Background, "anthropic"),
        "async by default; a bare id the catalog knows is a model"
    );
    assert_eq!(job("ASYNC fix it").unwrap().mode, Mode::Background);

    let plain = job("src/parser.rs is broken, fix it").unwrap();
    assert_eq!(
        (plain.model, plain.prompt.as_str()),
        (None, "src/parser.rs is broken, fix it"),
        "a path is not a provider"
    );
    assert_eq!(
        job("one two three four five six seven").unwrap().description,
        "one two three four five six..."
    );
    assert!(job("anthropic/claude-nothing look").unwrap_err().contains("no model"));
    assert!(job("seq anthropic/claude-haiku-4-5").unwrap_err().contains(USAGE));
}

#[test]
fn only_a_queued_prompt_reading_the_command_is_its_job() {
    let queued = |text: &str| queued_arguments(&waiting(text));

    assert_eq!(queued("seq look").as_deref(), Some("seq look"));
    assert_eq!(
        queued_arguments(&prompt("/subagents look")),
        None,
        "another word is not the command"
    );
    assert_eq!(queued_arguments(&prompt("please /subagent look")), None);
}

#[tokio::test]
async fn async_starts_a_user_worker_and_notes_it_without_a_turn_until_its_result_wakes_the_conversation() {
    let h = harness().await;
    select_model(&h);
    h.provider.push(text("worker found it")).push(text("parent read it"));

    let started = h
        .engine
        .start_subagent(&h.session.id, "investigate the parser")
        .await
        .unwrap();

    assert!(started.receipt.is_some() && started.queued.is_none());
    assert!(
        !h.engine.turns.is_running(&h.session.id),
        "the note alone starts no turn"
    );
    let note = &transcript(&h)[0];
    assert_eq!(note.info.role, Role::User);
    assert!(matches!(
        &note.parts[1].part,
        Part::Command { name, arguments } if name == NAME && arguments == "investigate the parser"
    ));
    let task = h.engine.store.tasks_of(&h.session.id).unwrap().remove(0);
    assert_eq!(
        (task.mode, task.agent.as_str(), task.reason.as_str()),
        (Mode::Background, AGENT, USER_REASON)
    );
    let Part::Text { text: said } = &note.parts[0].part else {
        panic!("{:?}", note.parts[0].part)
    };
    assert!(
        said.contains("you did not launch it") && said.contains(&task.id),
        "{said}"
    );

    until("the result is handed over", || {
        h.engine.store.task(&task.id).unwrap().unwrap().delivered
    })
    .await;
    until_idle(&h).await;

    assert!(
        opens_with(&h, 0, "investigate the parser"),
        "the worker gets the prompt alone"
    );
    let parent = sent(&h, 1);
    assert!(
        parent.contains("you did not launch it") && parent.contains("worker found it"),
        "{parent}"
    );
    assert_eq!(
        h.engine.store.task(&task.id).unwrap().unwrap().state,
        TaskState::Replied
    );
}

#[tokio::test]
async fn async_while_a_turn_runs_reaches_it_as_a_steer() {
    let h = harness().await;
    select_model(&h);
    h.provider
        .push_slow(Duration::from_millis(600), text("working"))
        .push(text("worker found it"))
        .push(text("parent read both"));
    h.engine.submit(&h.session.id, prompt("start")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    h.engine
        .start_subagent(&h.session.id, "async check the tests")
        .await
        .unwrap();
    until("the worker replies", || {
        h.engine.store.tasks_of(&h.session.id).unwrap()[0].state == TaskState::Replied
    })
    .await;
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().len();
    assert_eq!(
        requests, 3,
        "the running turn took the note and the result, no extra turn"
    );
    let parent = sent(&h, 2);
    assert!(
        parent.contains("you did not launch it") && parent.contains("worker found it"),
        "{parent}"
    );
}

#[tokio::test]
async fn seq_runs_the_job_as_the_first_call_of_a_turn_that_carries_on_from_its_result() {
    let h = harness().await;
    select_model(&h);
    h.provider.push(text("worker answer")).push(text("parent answer"));

    let started = h.engine.start_subagent(&h.session.id, "seq look at src").await.unwrap();
    assert!(started.receipt.is_some());
    until_idle(&h).await;

    let task = h.engine.store.tasks_of(&h.session.id).unwrap().remove(0);
    assert_eq!((task.mode, task.agent.as_str()), (Mode::Foreground, AGENT));
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.name, "task");
    assert_eq!(
        call.metadata.and_then(|metadata| metadata.engine_command.as_deref()),
        Some(NAME)
    );
    assert!(opens_with(&h, 0, "look at src"));
    let parent = sent(&h, 1);
    assert!(
        parent.contains("made for them") && parent.contains("worker answer"),
        "{parent}"
    );
}

#[tokio::test]
async fn seq_while_a_turn_runs_waits_in_the_queue_and_starts_the_next_turn() {
    let h = harness().await;
    select_model(&h);
    h.provider
        .push_slow(Duration::from_millis(400), text("working"))
        .push(text("worker answer"))
        .push(text("parent answer"));
    h.engine.submit(&h.session.id, prompt("start")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started = h
        .engine
        .start_subagent(&h.session.id, "seq check the tests")
        .await
        .unwrap();
    let queued = started.queued.expect("queued behind the turn");
    assert_eq!(queued_arguments(&queued.prompt).as_deref(), Some("seq check the tests"));
    assert!(
        h.engine.store.tasks_of(&h.session.id).unwrap().is_empty(),
        "nothing starts before the turn ends"
    );

    until("the queued job ran", || {
        h.engine.store.queued_prompts(&h.session.id).unwrap().is_empty()
            && h.provider.requests.lock().unwrap().len() == 3
    })
    .await;
    until_idle(&h).await;

    assert!(opens_with(&h, 1, "check the tests"));
    assert!(sent(&h, 2).contains("worker answer"));
    assert_eq!(
        h.engine.store.tasks_of(&h.session.id).unwrap()[0].mode,
        Mode::Foreground
    );
}

#[tokio::test]
async fn an_unknown_model_or_a_subagent_transcript_starts_nothing() {
    let h = harness().await;
    select_model(&h);

    let refused = h
        .engine
        .start_subagent(&h.session.id, "anthropic/claude-nothing look")
        .await
        .unwrap_err();
    assert!(matches!(&refused, CommandError::Invalid(reason) if reason.contains("no model")));
    assert!(transcript(&h).is_empty() && h.engine.store.tasks_of(&h.session.id).unwrap().is_empty());

    h.provider.push(text("worker")).push(text("parent"));
    h.engine.start_subagent(&h.session.id, "seq look").await.unwrap();
    until_idle(&h).await;
    let worker = h.engine.store.tasks_of(&h.session.id).unwrap().remove(0);
    assert!(matches!(
        h.engine.start_subagent(&worker.session_id, "look").await,
        Err(CommandError::Invalid(_))
    ));
}

#[tokio::test]
async fn sending_a_queued_seq_job_now_runs_it_beside_the_turn() {
    let h = harness().await;
    select_model(&h);
    h.provider
        .push_slow(Duration::from_millis(600), text("working"))
        .push(text("worker found it"))
        .push(text("parent read both"));
    h.engine.submit(&h.session.id, prompt("start")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let queued = h
        .engine
        .start_subagent(&h.session.id, "seq check the tests")
        .await
        .unwrap()
        .queued
        .unwrap();

    let receipt = h.engine.send_queued_now(&queued.id).await.unwrap();
    assert!(receipt.is_some());
    assert!(h.engine.store.queued_prompts(&h.session.id).unwrap().is_empty());
    until("the worker replies", || {
        h.engine.store.tasks_of(&h.session.id).unwrap()[0].state == TaskState::Replied
    })
    .await;
    until_idle(&h).await;

    let task = h.engine.store.tasks_of(&h.session.id).unwrap().remove(0);
    assert_eq!((task.mode, task.reason.as_str()), (Mode::Background, USER_REASON));
    assert!(sent(&h, 2).contains("worker found it"));
}

fn nothing_started(h: &Harness) -> bool {
    transcript(h).is_empty() && h.engine.store.tasks_of(&h.session.id).unwrap().is_empty()
}

#[tokio::test]
async fn async_from_a_read_only_agent_is_refused_as_seq_is() {
    let h = harness().await;
    select_model(&h);
    h.engine
        .store
        .update_session(&h.session.id, None, None, Some("plan"))
        .unwrap();

    let refused = h
        .engine
        .start_subagent(&h.session.id, "fix the parser")
        .await
        .unwrap_err();
    let CommandError::Turn(TurnError::Refused(reason)) = refused else {
        panic!("{refused:?}")
    };
    assert!(nothing_started(&h));

    h.provider.push(text("understood"));
    h.engine
        .start_subagent(&h.session.id, "seq fix the parser")
        .await
        .unwrap();
    until_idle(&h).await;
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.output, Some(reason.as_str()), "the same words either way");
    assert!(h.engine.store.tasks_of(&h.session.id).unwrap().is_empty());
}

#[tokio::test]
async fn a_rule_denying_task_refuses_async_and_an_ask_rule_lets_the_users_command_through() {
    use crate::permission::{Policy, Rule};

    let h = harness().await;
    select_model(&h);
    let rule = |decision| Policy {
        rules: vec![Rule {
            kind: "task".into(),
            pattern: AGENT.into(),
            decision,
        }],
        ..Default::default()
    };
    h.engine.permissions.set_policy(rule(Decision::Deny));

    let refused = h.engine.start_subagent(&h.session.id, "look").await.unwrap_err();
    assert!(matches!(
        &refused,
        CommandError::Turn(TurnError::Refused(reason)) if reason == "A permission rule forbids this call."
    ));
    assert!(nothing_started(&h));

    h.engine.permissions.set_policy(rule(Decision::Ask));
    h.provider.push(text("worker found it")).push(text("parent read it"));
    h.engine.start_subagent(&h.session.id, "look").await.unwrap();
    assert!(h.engine.permissions.pending().is_empty(), "the user is not asked again");
    until_idle(&h).await;
    assert_eq!(h.engine.store.tasks_of(&h.session.id).unwrap().len(), 1);
}

#[tokio::test]
async fn an_archived_conversation_starts_nothing() {
    let h = harness().await;
    select_model(&h);
    h.engine.store.set_session_archived(&h.session.id, true).unwrap();

    for typed in ["look", "seq look"] {
        let refused = h.engine.start_subagent(&h.session.id, typed).await.unwrap_err();
        assert!(matches!(&refused, CommandError::Invalid(reason) if reason.contains("archived")));
    }
    assert!(nothing_started(&h));
}

#[tokio::test]
async fn with_background_tasks_off_each_refusal_says_what_works_instead() {
    let h = harness().await;
    select_model(&h);
    h.engine
        .store
        .set_setting(crate::session::tasks::BACKGROUND_TASKS_KEY, &false)
        .unwrap();

    let refused = h.engine.start_subagent(&h.session.id, "look").await.unwrap_err();
    assert!(matches!(&refused, CommandError::Invalid(reason) if reason.contains("`/subagent seq`")));

    h.provider.push_slow(Duration::from_millis(400), text("working"));
    h.engine.submit(&h.session.id, prompt("start")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let queued = h
        .engine
        .start_subagent(&h.session.id, "seq look")
        .await
        .unwrap()
        .queued
        .unwrap();
    let refused = h.engine.send_queued_now(&queued.id).await.unwrap_err();
    let said = refused.to_string();
    assert!(
        said.contains("once the current turn ends") && !said.contains("/subagent seq"),
        "{said}"
    );
    until_idle(&h).await;
}
