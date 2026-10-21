use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use serde_json::{Value, json};

use super::{ImageGeneration, ImageRequest, SETTINGS_KEY, Size, TEST_BASES};
use crate::Engine;
use crate::llm::Credential;
use crate::tool::image::Image;

mod cloud;
mod local;
mod plan;

/// What a stand-in provider was sent: the path, the key header, the headers and the body (a form as text).
#[derive(Clone, Debug)]
pub(crate) struct Asked {
    pub path: String,
    pub key: String,
    pub headers: HeaderMap,
    pub body: Value,
}

pub(crate) type Log = Arc<Mutex<Vec<Asked>>>;

/// The stand-in server's state: what it was asked, where it is, and how the ChatGPT plan answers.
#[derive(Clone)]
struct Server {
    log: Log,
    url: Arc<Mutex<String>>,
    plan_spent: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
}

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
    let authorization = header("authorization");
    let bearer = authorization.trim_start_matches("Bearer ").trim_start_matches("Key ");

    [bearer, header("x-goog-api-key"), header("x-key"), header("api-key")]
        .into_iter()
        .find(|key| !key.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn note(server: &Server, path: String, headers: &HeaderMap, body: &Bytes) {
    let body = serde_json::from_slice(body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into()));
    server.log.lock().unwrap().push(Asked {
        path,
        key: key(headers),
        headers: headers.clone(),
        body,
    });
}

fn base(server: &Server) -> String {
    server.url.lock().unwrap().clone()
}

/// OpenAI's Images API, also standing in for the ChatGPT plan's route and an OpenAI-compatible server.
async fn openai(State(server): State<Server>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    note(&server, uri.path().to_string(), &headers, &body);
    if uri.path().starts_with("/chatgpt/") && server.plan_spent.load(Ordering::SeqCst) {
        let limit =
            json!({ "error": { "message": "You've reached your usage limit.", "type": "usage_limit_reached" } });
        return (StatusCode::TOO_MANY_REQUESTS, Json(limit)).into_response();
    }
    let usage = json!({
        "input_tokens": 50,
        "input_tokens_details": { "text_tokens": 10, "image_tokens": 40 },
        "output_tokens": 1000,
        "output_tokens_details": { "image_tokens": 1000, "text_tokens": 0 }
    });

    Json(json!({ "data": [{ "b64_json": png64() }], "usage": usage })).into_response()
}

async fn google(
    State(server): State<Server>,
    Path(model): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/google/models/{model}"), &headers, &body);
    let parts = json!([{ "text": "Here it is." }, { "inlineData": { "mimeType": "image/png", "data": png64() } }]);
    let usage = json!({
        "promptTokenCount": 100,
        "candidatesTokenCount": 1124,
        "candidatesTokensDetails": [{ "modality": "IMAGE", "tokenCount": 1120 }]
    });

    Json(json!({ "candidates": [{ "content": { "parts": parts } }], "usageMetadata": usage }))
}

async fn xai(State(server): State<Server>, Path(action): Path<String>, headers: HeaderMap, body: Bytes) -> Json<Value> {
    note(&server, format!("/xai/images/{action}"), &headers, &body);
    let image = json!({ "b64_json": png64(), "mime_type": "image/png", "revised_prompt": "a red square, centred" });

    Json(json!({ "data": [image], "usage": { "cost_in_usd_ticks": 200_000_000u64 } }))
}

async fn recraft(
    State(server): State<Server>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/recraft/images/{action}"), &headers, &body);
    match action.as_str() {
        "imageToImage" => {
            Json(json!({ "data": [{ "url": format!("{}/files/red.png", base(&server)) }], "credits": 40 }))
        }
        _ => Json(json!({ "data": [{ "b64_json": png64() }], "credits": 35 })),
    }
}

async fn flux_submit(
    State(server): State<Server>,
    Path(model): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/bfl/{model}"), &headers, &body);
    let polling = format!("{}/bfl/poll/req_1", base(&server));

    Json(json!({ "id": "req_1", "polling_url": polling, "cost": 3.0 }))
}

async fn flux_poll(State(server): State<Server>, headers: HeaderMap) -> Json<Value> {
    note(&server, "/bfl/poll".into(), &headers, &Bytes::new());
    if server.polls.fetch_add(1, Ordering::SeqCst) == 0 {
        return Json(json!({ "status": "Pending" }));
    }

    Json(json!({ "status": "Ready", "result": { "sample": format!("{}/files/red.png", base(&server)) } }))
}

async fn stability(
    State(server): State<Server>,
    Path(endpoint): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/stability/{endpoint}"), &headers, &body);

    Json(json!({ "image": png64(), "finish_reason": "SUCCESS", "seed": 1 }))
}

async fn ideogram(
    State(server): State<Server>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/ideogram/{action}"), &headers, &body);
    let url = format!("{}/files/red.png", base(&server));

    Json(json!({ "data": [{ "url": url, "is_image_safe": true, "prompt": "a red square, flat" }] }))
}

async fn fal(
    State(server): State<Server>,
    Path(endpoint): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/fal/{endpoint}"), &headers, &body);

    Json(json!({ "images": [{ "url": format!("data:image/png;base64,{}", png64()), "content_type": "image/png" }] }))
}

async fn file() -> impl IntoResponse {
    ([("content-type", "image/png")], tiny_png())
}

/// Serves every provider under its id (`/openai`, `/recraft`, ...) and the plan under `/chatgpt`, which is
/// where tests point each, and the image files answers name under `/files`.
pub(crate) async fn serve(engine: &Engine) -> Log {
    serve_with(engine).await.log
}

async fn serve_with(engine: &Engine) -> Server {
    let server = Server {
        log: Log::default(),
        url: Arc::default(),
        plan_spent: Arc::default(),
        polls: Arc::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    *server.url.lock().unwrap() = url.clone();
    let app = axum::Router::new()
        .route("/openai/images/{action}", post(openai))
        .route("/chatgpt/images/{action}", post(openai))
        .route("/compatible/images/{action}", post(openai))
        .route("/google/models/{model}", post(google))
        .route("/xai/images/{action}", post(xai))
        .route("/recraft/images/{action}", post(recraft))
        .route("/bfl/{model}", post(flux_submit))
        .route("/bfl/poll/{id}", get(flux_poll))
        .route("/stability/v2beta/stable-image/generate/{endpoint}", post(stability))
        .route("/ideogram/v1/ideogram-v3/{action}", post(ideogram))
        .route("/fal/{*endpoint}", post(fal))
        .route("/files/red.png", get(file))
        .merge(local::routes())
        .with_state(server.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TEST_BASES.lock().unwrap().push((engine.data_dir.clone(), url));

    server
}

/// Chooses `provider` and gives it what pays: a key in Settings > Providers for a catalog provider, a key
/// saved for images for the others, nothing for a server.
pub(crate) fn choose(engine: &Engine, provider: &str, model: &str) {
    let settings = ImageGeneration {
        provider: provider.into(),
        model: model.into(),
        ..ImageGeneration::default()
    };
    engine.store.set_setting(SETTINGS_KEY, &settings).unwrap();
    match super::providers::find(provider).unwrap().access {
        super::Access::Catalog => {
            let key = Credential::ApiKey {
                key: format!("{provider}-key"),
            };
            engine.credentials.set(provider, &key).unwrap();
        }
        super::Access::Key { .. } => engine.set_image_key(provider, &format!("{provider}-key")).unwrap(),
        super::Access::Server { .. } => {}
    }
}

async fn engine() -> (Arc<Engine>, Server) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = crate::platform::temp::test_run().join(format!("drift-imagegen-{}", crate::random_hex(4)));
    let options = crate::Options {
        file_credentials: true,
        ..Default::default()
    };
    let engine = Engine::open_with(&directory, options).unwrap();
    let server = serve_with(&engine).await;

    (engine, server)
}

fn request<'a>(prompt: &'a str, size: &str, references: Vec<Image>) -> ImageRequest<'a> {
    ImageRequest {
        prompt,
        size: Size::parse(size).unwrap(),
        references,
    }
}

fn reference() -> Image {
    Image::from_bytes("image/png", &tiny_png())
}

fn asked(server: &Server) -> Vec<Asked> {
    server.log.lock().unwrap().clone()
}
