use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::{Hit, WebSearch};
use crate::Engine;

/// What the stand-in providers do: refuse a key, rate limit Firecrawl, and count who was asked.
#[derive(Default)]
struct Behaviour {
    refused_key: Mutex<String>,
    firecrawl_busy: AtomicBool,
    asked: Mutex<Vec<String>>,
}

fn key(headers: &HeaderMap) -> String {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };
    let bearer = header("authorization").trim_start_matches("Bearer ");

    if bearer.is_empty() { header("x-api-key") } else { bearer }.to_string()
}

async fn firecrawl(State(behaviour): State<Arc<Behaviour>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let key = key(&headers);
    behaviour.asked.lock().unwrap().push(format!("firecrawl:{key}"));
    if *behaviour.refused_key.lock().unwrap() == key {
        return (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({ "error": "Payment required" })),
        )
            .into_response();
    }
    if behaviour.firecrawl_busy.load(Ordering::SeqCst) {
        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1")], "slow down").into_response();
    }
    let page = json!({ "url": "https://fire.example", "title": "Fire", "description": body["query"] });

    Json(json!({ "success": true, "data": { "web": [page] } })).into_response()
}

async fn exa(State(behaviour): State<Arc<Behaviour>>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    behaviour.asked.lock().unwrap().push(format!("exa:{}", key(&headers)));
    assert_eq!(body["contents"]["highlights"], true);

    Json(json!({ "results": [{ "url": "https://exa.example", "title": "Exa", "highlights": ["one", "two"] }] }))
}

async fn tavily(State(behaviour): State<Arc<Behaviour>>, headers: HeaderMap) -> Json<Value> {
    behaviour
        .asked
        .lock()
        .unwrap()
        .push(format!("tavily:{}", key(&headers)));

    Json(json!({ "results": [{ "url": "https://tavily.example", "title": "Tavily", "content": "found" }] }))
}

/// A SearXNG instance's JSON answer, for the plugin.
async fn searxng(axum::extract::Query(query): axum::extract::Query<Vec<(String, String)>>) -> Json<Value> {
    let asked = query
        .iter()
        .find(|(name, _)| name == "q")
        .map(|(_, value)| value.clone());

    Json(json!({ "results": [{ "url": "https://searx.example", "title": "Searx", "content": asked }] }))
}

async fn serve(behaviour: Arc<Behaviour>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route("/firecrawl", post(firecrawl))
        .route("/exa", post(exa))
        .route("/tavily", post(tavily))
        .route("/search", get(searxng))
        .with_state(behaviour);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    url
}

async fn engine(behaviour: &Arc<Behaviour>) -> (Arc<Engine>, String) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let directory = std::env::temp_dir().join(format!("drift-websearch-{}", crate::random_hex(4)));
    let options = crate::Options {
        file_credentials: true,
        ..Default::default()
    };
    let engine = Engine::open_with(&directory, options).unwrap();
    let url = serve(behaviour.clone()).await;
    *engine.search_routing.base.lock().unwrap() = Some(url.clone());

    (engine, url)
}

fn choose(engine: &Engine, provider: &str, order: &[&str]) {
    let settings = WebSearch {
        provider: provider.into(),
        order: order.iter().map(ToString::to_string).collect(),
    };
    engine.store.set_setting(super::SETTINGS_KEY, &settings).unwrap();
}

fn asked(behaviour: &Behaviour, who: &str) -> usize {
    behaviour
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|asked| *asked == who)
        .count()
}

#[tokio::test]
async fn a_refused_key_is_set_aside_until_keys_change() {
    let behaviour = Arc::new(Behaviour::default());
    *behaviour.refused_key.lock().unwrap() = "spent".into();
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("firecrawl", "spent").unwrap();
    engine.add_search_key("firecrawl", "fresh").unwrap();
    choose(&engine, "firecrawl", &[]);

    let (provider, hits) = engine.web_search("s1", "rust", 5).await.unwrap();
    assert_eq!(provider, "firecrawl");
    assert_eq!(
        hits,
        [Hit {
            url: "https://fire.example".into(),
            title: "Fire".into(),
            snippet: "rust".into()
        }]
    );
    engine.web_search("s1", "rust", 5).await.unwrap();
    assert_eq!(asked(&behaviour, "firecrawl:spent"), 1, "a 402 sets the key aside");
    assert_eq!(asked(&behaviour, "firecrawl:fresh"), 2);

    engine.add_search_key("firecrawl", "spare").unwrap();
    engine.web_search("s1", "rust", 5).await.unwrap();
    assert_eq!(
        asked(&behaviour, "firecrawl:spent"),
        2,
        "a new key tries the set-aside ones again"
    );
    let listed = engine.search_providers();
    assert_eq!(listed[0].keys, ["...pent", "...resh", "...pare"]);
}

#[tokio::test]
async fn a_cooling_provider_hands_over_and_comes_back() {
    let behaviour = Arc::new(Behaviour::default());
    behaviour.firecrawl_busy.store(true, Ordering::SeqCst);
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("firecrawl", "f1").unwrap();
    engine.add_search_key("exa", "e1").unwrap();
    choose(&engine, "auto", &["firecrawl", "exa"]);

    let (provider, hits) = engine.web_search("s1", "rust", 5).await.unwrap();
    assert_eq!(provider, "exa");
    assert_eq!(hits[0].snippet, "one two");
    behaviour.firecrawl_busy.store(false, Ordering::SeqCst);

    let (provider, _) = engine.web_search("s2", "rust", 5).await.unwrap();
    assert_eq!(provider, "exa", "firecrawl's key still cools");
    assert_eq!(asked(&behaviour, "firecrawl:f1"), 1);

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (provider, _) = engine.web_search("s3", "rust", 5).await.unwrap();
    assert_eq!(provider, "firecrawl", "back after its Retry-After");
    let (provider, _) = engine.web_search("s1", "rust", 5).await.unwrap();
    assert_eq!(provider, "exa", "a session keeps the provider that last answered it");
}

#[tokio::test]
async fn tavily_answers_in_its_own_shape() {
    let behaviour = Arc::new(Behaviour::default());
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("tavily", "t1").unwrap();
    choose(&engine, "tavily", &[]);

    let (_, hits) = engine.web_search("s1", "rust", 5).await.unwrap();
    assert_eq!(hits[0].url, "https://tavily.example");
    assert_eq!(hits[0].snippet, "found");
    assert_eq!(asked(&behaviour, "tavily:t1"), 1);
}

#[tokio::test]
async fn off_removes_the_tool_and_refuses_a_search() {
    let behaviour = Arc::new(Behaviour::default());
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("exa", "e1").unwrap();
    choose(&engine, "exa", &[]);
    assert!(engine.web_search_available());

    choose(&engine, "off", &[]);
    assert!(!engine.web_search_available());
    assert!(engine.web_search("s1", "rust", 5).await.is_err());
    assert!(
        engine.add_search_key("searxng", "k").is_err(),
        "only built-ins take keys"
    );
}

#[cfg(feature = "wasm-plugins")]
#[tokio::test]
async fn a_plugin_provider_answers() {
    let Some(path) = crate::hook::wasm::tests::built("searxng") else {
        return;
    };
    let behaviour = Arc::new(Behaviour::default());
    let (engine, url) = engine(&behaviour).await;
    let cache = std::env::temp_dir().join(format!("drift-plugin-cache-{}", crate::random_hex(4)));
    let listed = crate::hook::Listed {
        entry: "plugins/searxng.wasm".into(),
        path: Ok(path),
        config: json!({ "url": url }),
    };
    let loaded = engine
        .hooks
        .load(&cache, vec![listed], &[], Arc::downgrade(&engine))
        .await;
    assert_eq!(loaded[0].error, None);
    choose(&engine, "searxng", &[]);

    let plugin = engine
        .search_providers()
        .into_iter()
        .find(|provider| provider.id == "searxng");
    assert!(plugin.is_some_and(|provider| !provider.built_in && provider.name == "SearXNG"));
    let (provider, hits) = engine.web_search("s1", "rust & wasm", 5).await.unwrap();
    assert_eq!(provider, "searxng");
    assert_eq!(hits[0].url, "https://searx.example");
    assert_eq!(hits[0].snippet, "rust & wasm", "the query reaches the instance intact");
}
