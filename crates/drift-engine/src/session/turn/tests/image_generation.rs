use crate::imagegen::tests::{choose, serve, tiny_png};
use crate::llm::Block;

use super::*;

fn offered(h: &Harness, at: usize) -> bool {
    let requests = h.provider.requests.lock().unwrap();
    requests[at].tools.iter().any(|tool| tool.name == "generate_image")
}

#[tokio::test]
async fn generate_image_is_offered_only_once_turned_on_with_a_key() {
    let h = harness().await;
    h.provider.push(text("one")).push(text("two"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    assert!(!offered(&h, 0), "off until enabled");

    choose(&h.engine, "openai", "");
    h.engine.submit(&h.session.id, prompt("second")).await.await_ok();
    until_idle(&h).await;
    assert!(offered(&h, 1));
}

#[tokio::test]
async fn an_approved_image_lands_where_asked_reaches_the_model_and_is_charged() {
    let h = harness().await;
    let log = serve(&h.engine).await;
    choose(&h.engine, "openai", "");
    std::fs::write(h._dir.join("ws/start.png"), tiny_png()).unwrap();
    let mut events = h.engine.hub.attach(None).rx;
    let input = r#"{"prompt": "a red square", "path": "art/red.jpg", "references": ["start.png"]}"#;
    h.provider.push(tool_call("generate_image", input)).push(text("done"));
    h.engine.submit(&h.session.id, prompt("draw")).await.await_ok();

    let ask = next_ask(&mut events).await;
    assert_eq!(
        (ask.ask.kind.as_str(), ask.ask.pattern.as_str()),
        ("generate_image", "a red square"),
        "every call is billed, so it asks with no rule set"
    );
    assert!(ask.ask.title.contains("OpenAI"));
    reply_permission(&h, &ask.id, Reply::Once);
    until_idle(&h).await;

    let saved = std::fs::read(h._dir.join("ws/art/red.jpg")).unwrap();
    assert_eq!(
        crate::tool::image::sniff(&saved),
        Some("image/jpeg"),
        "saved in the path's format"
    );
    assert_eq!(log.lock().unwrap()[0].path, "/openai/images/edits");
    let messages = transcript(&h);
    let call = tool(&messages[1].parts[0]);
    assert_eq!(call.status, ToolStatus::Done);
    assert!(call.output.unwrap().contains("It cost $0.030"), "{:?}", call.output);
    let images = crate::tool::image::stored(call.metadata);
    assert_eq!(images.len(), 1, "the image is kept for the model and the transcript");
    let image_cost = (10.0 * 5.0 + 40.0 * 8.0 + 1000.0 * 30.0) / 1e6;
    assert!(
        messages[1].info.cost >= image_cost,
        "charged to the reply that made the call"
    );

    let requests = h.provider.requests.lock().unwrap().clone();
    let shown = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .any(|block| matches!(block, Block::Image { .. }));
    assert!(shown, "the model sees what it made");
}

#[tokio::test]
async fn a_path_outside_the_workspace_and_scratch_is_refused_before_anything_is_sent() {
    let h = harness().await;
    let log = serve(&h.engine).await;
    choose(&h.engine, "xai", "");
    let input = r#"{"prompt": "a red square", "path": "../outside.png"}"#;
    h.provider.push(tool_call("generate_image", input)).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("draw")).await.await_ok();
    until_idle(&h).await;

    let call = tool(&transcript(&h)[1].parts[0]).output.unwrap().to_string();
    assert!(
        call.contains("outside the workspace and the scratch directory"),
        "{call}"
    );
    assert!(log.lock().unwrap().is_empty());
    assert!(!h._dir.join("outside.png").exists());
}

#[tokio::test]
async fn without_a_path_the_image_goes_to_scratch_as_a_png() {
    let h = harness().await;
    serve(&h.engine).await;
    choose(&h.engine, "google", "");
    rule(&h, "generate_image", "*", Decision::Allow);
    let call_id = format!("toolu_{}", crate::random_hex(4));
    let chunks = [
        call_block(
            &call_id,
            "generate_image",
            r#"{"prompt": "a red square", "size": "1:1"}"#,
        ),
        vec![Chunk::Stop(StopReason::ToolUse)],
    ]
    .concat();
    h.provider.push(chunks).push(text("ok"));
    h.engine.submit(&h.session.id, prompt("draw")).await.await_ok();
    until_idle(&h).await;

    let path = crate::tool::scratch_dir().join("images").join(format!("{call_id}.png"));
    let saved = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(saved, tiny_png());
}
