//! Two short requests against the real Codex backend, run by hand: `cargo test -p drift-engine --lib live_codex -- --ignored`.

use super::*;

#[tokio::test]
#[ignore = "uses the live ChatGPT sign-in for two short Responses requests"]
async fn live_codex_continues_a_conversation_over_one_websocket() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // The sign-in is only read; this never refreshes or writes credentials.
    let data = std::path::PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA")).join("dev.drift.app");
    let credential = crate::llm::credentials::Credentials::open(&data, false)
        .get("openai")
        .expect("an OpenAI sign-in");
    assert!(!credential.is_expired(), "sign in to Drift again first");

    let provider = OpenAi {
        websockets: Arc::default(),
        ..OpenAi::default()
    };

    let mut first = request();
    first.cache_key = Some(format!("websocket_probe_{}", crate::random_hex(4)));
    first.messages = vec![text(Role::User, "Reply with exactly the word ok.")];
    let reply = collect(&provider, &first, &credential).await;

    let mut next = first.clone();
    next.messages.push(ChatMessage {
        role: Role::Assistant,
        blocks: replayed(reply),
    });
    next.messages
        .push(text(Role::User, "Reply with exactly the word ok again."));
    let chunks = collect(&provider, &next, &credential).await;

    let endpoint = format!("{}/responses", crate::llm::openai::CODEX_BASE_URL);
    assert!(chunks.iter().all(Result::is_ok));
    assert!(!provider.websockets.unsupported(&endpoint), "the upgrade was refused");
    assert_eq!(provider.websockets.entries.lock().unwrap().len(), 1);
}

/// The reply as a turn would replay it: its signed reasoning, then its text.
fn replayed(chunks: Vec<Result<Chunk, Error>>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut reasoning = String::new();
    let mut answer = String::new();

    for chunk in chunks {
        match chunk.unwrap() {
            Chunk::TextDelta(delta) => answer.push_str(&delta),
            Chunk::ReasoningDelta(delta) => reasoning.push_str(&delta),
            Chunk::ReasoningSignature(signature) => blocks.push(Block::Reasoning {
                text: std::mem::take(&mut reasoning),
                signature: Some(signature),
                redacted: None,
            }),
            _ => {}
        }
    }

    assert!(!answer.is_empty(), "the model said nothing");
    blocks.push(Block::Text(answer));

    blocks
}
