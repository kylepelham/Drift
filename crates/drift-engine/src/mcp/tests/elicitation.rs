use super::protocol::v2_http_server;
use super::*;

/// Connects the stand-in server that asks who is calling, under the name `remote`.
async fn asking_server(engine: &Arc<crate::Engine>) -> Arc<Mutex<Vec<super::protocol::Seen>>> {
    let (url, seen) = v2_http_server().await;
    let config = ServerConfig::Http {
        url,
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    saved(engine, "remote", &config).await;
    engine.connect_mcp_in("remote", Some(&here())).await.unwrap();

    seen
}

/// The next card asked, once it shows.
async fn card(engine: &crate::Engine) -> crate::question::Request {
    until("a card is asked", || !engine.questions.pending().is_empty()).await;
    engine.questions.pending().remove(0)
}

fn calls(seen: &Mutex<Vec<super::protocol::Seen>>) -> usize {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|request| request.rpc == "tools/call")
        .count()
}

#[tokio::test]
async fn a_form_is_asked_on_the_card_and_the_answer_reaches_the_server() {
    let engine = engine();
    let seen = asking_server(&engine).await;
    let ctx = context(&engine);
    let call = tool(&engine, "remote_echo");
    let run = call.run(&ctx, json!({ "text": "ask" }));

    let answering = async {
        let asked = card(&engine).await;
        assert_eq!(asked.session_id, "s", "asked in the session whose call it is");
        assert_eq!(asked.call_id, "c");
        assert!(!asked.is_async, "the call waits for it");
        assert_eq!(asked.questions[0].header, "remote", "the card names the server");
        assert!(asked.questions[0].question.starts_with("Who are you?"));
        engine
            .answer_question(&asked.id, Some(vec![vec!["Ada".into()]]))
            .await
            .unwrap();
    };
    let (output, ()) = tokio::join!(run, answering);

    let output = output.unwrap().output;
    assert!(output.contains(r#""action":"accept""#), "{output}");
    assert!(output.contains(r#""content":{"name":"Ada"}"#), "{output}");
    assert_eq!(calls(&seen), 2, "the call, then its retry carrying the answer");
}

#[tokio::test]
async fn a_bad_answer_is_asked_again_and_never_sent() {
    let engine = engine();
    let seen = asking_server(&engine).await;
    let ctx = context(&engine);
    let call = tool(&engine, "remote_echo");
    let run = call.run(&ctx, json!({ "text": "ask" }));

    let answering = async {
        let first = card(&engine).await;
        engine
            .answer_question(&first.id, Some(vec![vec!["A".into()]]))
            .await
            .unwrap();
        let again = card(&engine).await;
        assert!(
            again.questions[0]
                .question
                .starts_with("name: needs at least 2 characters"),
            "{}",
            again.questions[0].question
        );
        assert_eq!(calls(&seen), 1, "nothing went to the server yet");
        engine
            .answer_question(&again.id, Some(vec![vec!["Ada".into()]]))
            .await
            .unwrap();
    };
    let (output, ()) = tokio::join!(run, answering);

    assert!(output.unwrap().output.contains(r#""name":"Ada""#));
}

#[tokio::test]
async fn a_dismissed_card_declines() {
    let engine = engine();
    asking_server(&engine).await;
    let ctx = context(&engine);
    let call = tool(&engine, "remote_echo");
    let run = call.run(&ctx, json!({ "text": "ask" }));

    let dismissing = async {
        let asked = card(&engine).await;
        engine.answer_question(&asked.id, None).await.unwrap();
    };
    let (output, ()) = tokio::join!(run, dismissing);

    let output = output.unwrap().output;
    assert!(output.contains(r#""action":"decline""#), "{output}");
    assert!(
        !output.contains("content"),
        "a declined form sends no content: {output}"
    );
}

#[tokio::test]
async fn stop_cancels_the_question_and_closes_the_card() {
    let engine = engine();
    asking_server(&engine).await;
    let ctx = context(&engine);
    let call = tool(&engine, "remote_echo");
    let run = call.run(&ctx, json!({ "text": "ask" }));

    let stopping = async {
        card(&engine).await;
        ctx.abort.cancel();
    };
    let (output, ()) = tokio::join!(run, stopping);

    assert!(output.is_err(), "the stopped call ends");
    until("the card closes", || engine.questions.pending().is_empty()).await;
}

#[tokio::test]
async fn a_question_from_no_call_is_declined_and_a_stopped_one_cancelled() {
    use rmcp::model::{ElicitRequestParams, ElicitationAction};

    let engine = engine();
    let form = || {
        serde_json::from_value::<ElicitRequestParams>(json!({
            "mode": "form",
            "message": "Who?",
            "requestedSchema": { "type": "object", "properties": {} },
        }))
        .unwrap()
    };
    let callers = Arc::new(super::super::elicit::Callers::default());
    let alone = super::super::elicit::answer(&callers, form()).await;
    assert_eq!(alone.action, ElicitationAction::Decline);

    let ctx = context(&engine);
    ctx.abort.cancel();
    let _entered = callers.enter(&ctx, "remote");
    let stopped = super::super::elicit::answer(&callers, form()).await;
    assert_eq!(stopped.action, ElicitationAction::Cancel);
}
