use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::post;
use base64::Engine as _;
use serde_json::{Value, json};

use super::{ImageGeneration, ImageRequest, SETTINGS_KEY, Size, TEST_BASES};
use crate::Engine;
use crate::llm::Credential;
use crate::tool::image::Image;

/// What a stand-in provider was sent: the path, the key header and the body.
#[derive(Clone, Debug)]
pub(crate) struct Asked {
    pub path: String,
    pub key: String,
    pub body: Value,
}

type Log = Arc<Mutex<Vec<Asked>>>;

pub(crate) fn tiny_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(2, 2, image::Rgb([200, 40, 40]))
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();

    bytes
}

fn png64() -> String {
    base64::engine::general_purpose::STANDARD.encode(tiny_png())
}

fn key(headers: &HeaderMap) -> String {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };
    let bearer = header("authorization").trim_start_matches("Bearer ");

    if bearer.is_empty() {
        header("x-goog-api-key")
    } else {
        bearer
    }
    .to_string()
}

fn note(log: &Log, path: String, headers: &HeaderMap, body: Value) {
    log.lock().unwrap().push(Asked {
        path,
        key: key(headers),
        body,
    });
}

async fn openai(
    State(log): State<Log>,
    Path(action): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    note(&log, format!("/openai/images/{action}"), &headers, body);
    let usage = json!({
        "input_tokens": 50,
        "input_tokens_details": { "text_tokens": 10, "image_tokens": 40 },
        "output_tokens": 1000,
        "output_tokens_details": { "image_tokens": 1000, "text_tokens": 0 }
    });

    Json(json!({ "data": [{ "b64_json": png64() }], "usage": usage }))
}

async fn google(
    State(log): State<Log>,
    Path(model): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    note(&log, format!("/google/models/{model}"), &headers, body);
    let parts = json!([{ "text": "Here it is." }, { "inlineData": { "mimeType": "image/png", "data": png64() } }]);
    let usage = json!({
        "promptTokenCount": 100,
        "candidatesTokenCount": 1124,
        "candidatesTokensDetails": [{ "modality": "IMAGE", "tokenCount": 1120 }]
    });

    Json(json!({ "candidates": [{ "content": { "parts": parts } }], "usageMetadata": usage }))
}

async fn xai(
    State(log): State<Log>,
    Path(action): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    note(&log, format!("/xai/images/{action}"), &headers, body);
    let image = json!({ "b64_json": png64(), "mime_type": "image/png", "revised_prompt": "a red square, centred" });

    Json(json!({ "data": [image], "usage": { "cost_in_usd_ticks": 200_000_000u64 } }))
}

/// Serves the three providers under `/openai`, `/google` and `/xai`, which is where tests point each.
pub(crate) async fn serve(engine: &Engine) -> Log {
    let log = Log::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route("/openai/images/{action}", post(openai))
        .route("/google/models/{model}", post(google))
        .route("/xai/images/{action}", post(xai))
        .with_state(log.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TEST_BASES.lock().unwrap().push((engine.data_dir.clone(), url));

    log
}

/// Points `engine`'s providers at the stand-in server, by their id as the path's first part.
pub(crate) fn choose(engine: &Engine, provider: &str, model: &str) {
    let settings = ImageGeneration {
        provider: provider.into(),
        model: model.into(),
        size: "auto".into(),
    };
    engine.store.set_setting(SETTINGS_KEY, &settings).unwrap();
    let key = Credential::ApiKey {
        key: format!("{provider}-key"),
    };
    engine.credentials.set(provider, &key).unwrap();
}

async fn engine() -> (Arc<Engine>, Log) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = std::env::temp_dir().join(format!("drift-imagegen-{}", crate::random_hex(4)));
    let options = crate::Options {
        file_credentials: true,
        ..Default::default()
    };
    let engine = Engine::open_with(&directory, options).unwrap();
    let log = serve(&engine).await;

    (engine, log)
}

fn request<'a>(prompt: &'a str, size: &str, references: Vec<Image>) -> ImageRequest<'a> {
    ImageRequest {
        prompt,
        size: Size::parse(size).unwrap(),
        references,
    }
}

#[tokio::test]
async fn openai_generates_and_edits_with_references_as_data_urls() {
    let (engine, log) = engine().await;
    choose(&engine, "openai", "");

    let made = engine
        .generate_image(request("a red square", "16:9", Vec::new()))
        .await
        .unwrap();
    assert_eq!((made.provider, made.model.as_str()), ("OpenAI", "gpt-image-2"));
    assert_eq!(made.image.mime, "image/png");
    let cost = (10.0 * 5.0 + 40.0 * 8.0 + 1000.0 * 30.0) / 1e6;
    assert!(
        (made.cost.unwrap() - cost).abs() < 1e-12,
        "priced from the answer's token counts"
    );

    let reference = Image::from_bytes("image/png", &tiny_png());
    engine
        .generate_image(request("bluer", "1024x1536", vec![reference.clone()]))
        .await
        .unwrap();

    let asked = log.lock().unwrap().clone();
    assert_eq!(asked[0].path, "/openai/images/generations");
    assert_eq!(asked[0].key, "openai-key");
    assert_eq!(
        asked[0].body,
        json!({ "model": "gpt-image-2", "prompt": "a red square", "n": 1, "output_format": "png", "size": "1536x1024" })
    );
    assert_eq!(asked[1].path, "/openai/images/edits");
    assert_eq!(asked[1].body["size"], "1024x1536");
    assert_eq!(
        asked[1].body["images"],
        json!([{ "image_url": format!("data:image/png;base64,{}", reference.base64) }])
    );
}

#[tokio::test]
async fn google_asks_generate_content_for_an_image_with_its_ratio_and_tier() {
    let (engine, log) = engine().await;
    choose(&engine, "google", "gemini-3-pro-image");
    let reference = Image::from_bytes("image/png", &tiny_png());

    let made = engine
        .generate_image(request("a red square", "2000x1100", vec![reference.clone()]))
        .await
        .unwrap();
    assert_eq!(made.image.bytes().unwrap(), tiny_png());
    assert_eq!(made.revised_prompt.as_deref(), Some("Here it is."));
    let cost = (100.0 * 2.0 + 4.0 * 12.0 + 1120.0 * 120.0) / 1e6;
    assert!((made.cost.unwrap() - cost).abs() < 1e-12);

    let asked = log.lock().unwrap().clone();
    assert_eq!(asked[0].path, "/google/models/gemini-3-pro-image:generateContent");
    assert_eq!(asked[0].key, "google-key");
    let parts = &asked[0].body["contents"][0]["parts"];
    assert_eq!(parts[0]["inlineData"]["data"], reference.base64.as_str());
    assert_eq!(parts[1]["text"], "a red square");
    assert_eq!(
        asked[0].body["generationConfig"],
        json!({ "responseModalities": ["TEXT", "IMAGE"], "imageConfig": { "aspectRatio": "16:9", "imageSize": "2K" } })
    );
}

#[tokio::test]
async fn xai_reports_its_own_cost_and_takes_one_image_or_several() {
    let (engine, log) = engine().await;
    choose(&engine, "xai", "");
    let reference = Image::from_bytes("image/png", &tiny_png());

    let made = engine
        .generate_image(request("a red square", "auto", Vec::new()))
        .await
        .unwrap();
    assert_eq!(made.model, "grok-imagine-image-2.0");
    assert_eq!(made.cost, Some(0.02), "cost_in_usd_ticks, ten billion to the dollar");
    assert_eq!(made.revised_prompt.as_deref(), Some("a red square, centred"));
    engine
        .generate_image(request("edit", "auto", vec![reference.clone()]))
        .await
        .unwrap();
    engine
        .generate_image(request("combine", "4:3", vec![reference.clone(), reference]))
        .await
        .unwrap();

    let asked = log.lock().unwrap().clone();
    assert_eq!(asked[0].path, "/xai/images/generations");
    assert_eq!(
        asked[0].body,
        json!({ "model": "grok-imagine-image-2.0", "prompt": "a red square", "n": 1, "response_format": "b64_json" })
    );
    assert_eq!(asked[1].path, "/xai/images/edits");
    assert_eq!(asked[1].body["image"]["type"], "image_url");
    assert!(
        asked[1].body["image"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert_eq!(asked[2].body["images"].as_array().unwrap().len(), 2);
    assert_eq!(asked[2].body["aspect_ratio"], "4:3");
}

#[tokio::test]
async fn off_or_a_subscription_sign_in_leaves_the_tool_out() {
    let (engine, _) = engine().await;
    assert_eq!(engine.image_settings().provider, "off", "off until enabled");
    assert!(!engine.image_generation_available());
    assert!(engine.generate_image(request("x", "auto", Vec::new())).await.is_err());

    let subscription = Credential::OAuth {
        access: "a".into(),
        refresh: "r".into(),
        expires_at: i64::MAX,
        account: None,
    };
    engine.credentials.set("xai", &subscription).unwrap();
    assert_eq!(
        engine.credentials.api_key("xai", &[]),
        None,
        "a sign-in pays for chat, not images"
    );

    choose(&engine, "openai", "");
    assert!(engine.image_generation_available());
    let listed = engine.image_providers();
    assert_eq!(listed[0].models[0], "gpt-image-2");
    assert!(listed[0].has_key);
}
