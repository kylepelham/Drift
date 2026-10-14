use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::request::body;
use super::*;
use crate::llm::catalog::Reasoning;
use crate::llm::tests::request;
use crate::llm::{Block, ChatMessage, Role, ToolSpec};

#[test]
fn either_form_of_the_address_reaches_the_server_root() {
    assert_eq!(root("http://127.0.0.1:11434/v1/"), "http://127.0.0.1:11434");
    assert_eq!(root("https://ollama.com/v1"), "https://ollama.com");
    assert_eq!(root("http://box:11434"), "http://box:11434");
}

#[test]
fn the_body_runs_the_model_at_its_window_with_its_tools_and_tuned_sampling() {
    let sent = body(&Request {
        context_window: Some(131_072),
        max_tokens: 8_000,
        temperature: Some(0.6),
        top_k: Some(20),
        tools: vec![ToolSpec {
            name: "read".into(),
            description: "Reads a file".into(),
            input_schema: json!({ "type": "object" }),
        }],
        ..request()
    });

    assert_eq!(sent["stream"], true);
    assert_eq!(sent["messages"][0], json!({ "role": "system", "content": "sys" }));
    assert_eq!(
        sent["options"],
        json!({ "num_ctx": 131_072, "num_predict": 8_000, "temperature": 0.6, "top_k": 20 })
    );
    assert_eq!(sent["tools"][0]["function"]["name"], "read");
    assert!(
        sent.get("think").is_none(),
        "no level and nothing to show leaves thinking to the model"
    );

    // With calls off no tools are offered, and an unknown window sends none.
    let quiet = body(&Request {
        no_tool_calls: true,
        ..Request {
            tools: vec![ToolSpec {
                name: "read".into(),
                description: String::new(),
                input_schema: json!({}),
            }],
            ..request()
        }
    });
    assert!(quiet.get("tools").is_none());
    assert!(quiet["options"].get("num_ctx").is_none());
}

#[test]
fn a_reasoning_level_becomes_think() {
    let think = |reasoning: Option<Reasoning>, show_thinking: bool| {
        body(&Request {
            reasoning,
            show_thinking,
            ..request()
        })
        .get("think")
        .cloned()
    };
    let level = |level: &str| Some(Reasoning::Effort { level: level.into() });

    assert_eq!(think(level("high"), false), Some(json!("high")));
    assert_eq!(think(level("none"), true), Some(json!(false)));
    assert_eq!(
        think(None, true),
        Some(json!(true)),
        "a reasoning model's thinking comes apart from its answer"
    );
    assert_eq!(think(None, false), None);
}

#[test]
fn history_carries_calls_by_object_results_by_tool_name_and_images_on_the_message() {
    let messages = vec![
        ChatMessage {
            role: Role::Assistant,
            blocks: vec![
                Block::Reasoning {
                    text: "Look first.".into(),
                    signature: None,
                    redacted: None,
                },
                Block::Text("Reading it.".into()),
                Block::ToolUse {
                    id: "call_1".into(),
                    name: "read".into(),
                    input: json!({ "filePath": "main.rs" }),
                },
            ],
        },
        ChatMessage {
            role: Role::User,
            blocks: vec![
                Block::ToolResult {
                    call_id: "call_1".into(),
                    content: "fn main() {}".into(),
                    is_error: false,
                },
                Block::Image {
                    mime: "image/png".into(),
                    base64: "iVBOR".into(),
                },
                Block::Text("What does it do?".into()),
            ],
        },
    ];
    let sent = body(&Request { messages, ..request() });

    assert_eq!(
        sent["messages"][1],
        json!({
            "role": "assistant",
            "content": "Reading it.",
            "thinking": "Look first.",
            "tool_calls": [{ "function": { "name": "read", "arguments": { "filePath": "main.rs" } } }]
        })
    );
    assert_eq!(
        sent["messages"][2],
        json!({ "role": "tool", "tool_name": "read", "content": "fn main() {}" }),
        "a result follows its call, named by the call's tool"
    );
    assert_eq!(
        sent["messages"][3],
        json!({ "role": "user", "content": "What does it do?", "images": ["iVBOR"] })
    );
}

/// What the fake server last received.
#[derive(Default)]
struct Seen {
    body: Option<Value>,
    authorization: Option<String>,
}

struct Fake {
    seen: Mutex<Seen>,
    reply: (u16, String),
}

async fn handle(State(fake): State<Arc<Fake>>, headers: HeaderMap, axum::Json(body): axum::Json<Value>) -> Response {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    *fake.seen.lock().unwrap() = Seen {
        body: Some(body),
        authorization,
    };

    let (status, text) = fake.reply.clone();
    if status != 200 {
        return Response::builder().status(status).body(Body::from(text)).unwrap();
    }

    // Seven-byte pieces split lines and multi-byte characters across reads.
    let pieces: Vec<Result<Vec<u8>, std::io::Error>> =
        text.as_bytes().chunks(7).map(|piece| Ok(piece.to_vec())).collect();
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(Body::from_stream(futures_util::stream::iter(pieces)))
        .unwrap()
}

async fn fake(status: u16, reply: &str) -> (Arc<Fake>, Ollama) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fake = Arc::new(Fake {
        seen: Mutex::default(),
        reply: (status, reply.into()),
    });
    let router = axum::Router::new()
        .route("/api/chat", axum::routing::post(handle))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    (fake, Ollama::new(&base))
}

fn lines(objects: &[Value]) -> String {
    objects.iter().map(|object| object.to_string() + "\n").collect()
}

async fn chunks_of(ollama: &Ollama, credential: &Credential) -> Result<Vec<Chunk>, Error> {
    let mut stream = ollama.stream(&request(), credential).await?;
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk?);
    }

    Ok(chunks)
}

fn key(key: &str) -> Credential {
    Credential::ApiKey { key: key.into() }
}

#[tokio::test]
async fn a_streamed_reply_becomes_thinking_text_and_whole_calls_with_its_usage() {
    let reply = lines(&[
        json!({ "message": { "role": "assistant", "content": "", "thinking": "Check the file " }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "", "thinking": "first." }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "Reading café.rs now." }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "", "tool_calls": [
            { "function": { "name": "read", "arguments": { "filePath": "main.rs" } } }
        ] }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "" }, "done": true, "done_reason": "stop",
            "prompt_eval_count": 1_200, "eval_count": 40 }),
    ]);
    let (fake, ollama) = fake(200, &reply).await;

    let chunks = chunks_of(&ollama, &key("none")).await.unwrap();

    let Some(Chunk::ToolUseStart { id, .. }) = chunks.iter().find(|chunk| matches!(chunk, Chunk::ToolUseStart { .. }))
    else {
        panic!("no call in {chunks:?}");
    };
    assert!(!id.is_empty(), "a call Ollama sends without an id gets one");
    let id = id.clone();
    assert_eq!(
        chunks,
        [
            Chunk::ReasoningStart,
            Chunk::ReasoningDelta("Check the file ".into()),
            Chunk::ReasoningDelta("first.".into()),
            Chunk::BlockStop,
            Chunk::TextStart,
            Chunk::TextDelta("Reading café.rs now.".into()),
            Chunk::BlockStop,
            Chunk::ToolUseStart {
                id,
                name: "read".into()
            },
            Chunk::ToolInputDelta(r#"{"filePath":"main.rs"}"#.into()),
            Chunk::BlockStop,
            Chunk::Usage(Usage {
                input: 1_200,
                output: 40,
                cache_read: 0,
                cache_write: 0,
            }),
            Chunk::Stop(StopReason::ToolUse),
        ]
    );

    // A local server is sent no key.
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen.authorization, None);
    assert_eq!(seen.body.as_ref().unwrap()["model"], "claude-sonnet-4-5");
}

#[tokio::test]
async fn ollama_cloud_is_sent_its_key_and_a_reply_cut_at_its_limit_says_so() {
    let reply = lines(&[
        json!({ "message": { "content": "Partly" }, "done": false }),
        json!({ "message": { "content": "" }, "done": true, "done_reason": "length", "prompt_eval_count": 9, "eval_count": 100 }),
    ]);
    let (fake, ollama) = fake(200, &reply).await;

    let chunks = chunks_of(&ollama, &key("ollama-key")).await.unwrap();

    assert_eq!(chunks.last(), Some(&Chunk::Stop(StopReason::MaxTokens)));
    assert_eq!(
        fake.seen.lock().unwrap().authorization.as_deref(),
        Some("Bearer ollama-key")
    );
}

#[tokio::test]
async fn refusals_and_failures_are_errors_the_engine_can_tell_apart() {
    let (_, missing) = fake(404, r#"{"error":"model 'qwen9' not found, try pulling it first"}"#).await;
    let Err(Error::Api { status, message, .. }) = chunks_of(&missing, &key("none")).await else {
        panic!("a missing model is a provider error");
    };
    assert_eq!(
        (status, message.as_str()),
        (404, "model 'qwen9' not found, try pulling it first")
    );

    let (_, unsigned) = fake(401, r#"{"error":"unauthorized"}"#).await;
    assert!(matches!(
        chunks_of(&unsigned, &key("bad")).await,
        Err(Error::Unauthenticated(words)) if words == "unauthorized"
    ));

    let (_, broken) = fake(
        200,
        &lines(&[json!({ "error": "the model failed to generate a response" })]),
    )
    .await;
    assert!(matches!(
        chunks_of(&broken, &key("none")).await,
        Err(Error::Api { status: crate::llm::STREAMED, message, .. }) if message.contains("failed to generate")
    ));
}

#[test]
fn both_routes_speak_the_native_api_at_their_roots() {
    let route = |id: &str, api: Option<&str>| match crate::llm::provider_for(id, api) {
        Some(crate::llm::Provider::Ollama(ollama)) => ollama.root,
        _ => panic!("{id} is not on the native route"),
    };

    assert_eq!(route("ollama", None), LOCAL_ROOT);
    assert_eq!(
        route("ollama-cloud", Some("https://ollama.com/v1")),
        CLOUD_ROOT,
        "models.dev names its /v1"
    );

    let cloud = &crate::llm::catalog::Catalog::bundled().providers["ollama-cloud"];
    assert_eq!(cloud.env, ["OLLAMA_API_KEY"]);
    assert!(cloud.models.values().any(|model| model.limit.context > 0));
}
