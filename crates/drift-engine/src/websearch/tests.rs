use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::{Hit, Query, WebSearch};
use crate::Engine;

/// What the stand-in providers do: refuse a key, rate limit Firecrawl, and count who was asked.
#[derive(Default)]
struct Behaviour {
    refused_key: Mutex<String>,
    firecrawl_busy: AtomicBool,
    asked: Mutex<Vec<String>>,
    /// The body of Firecrawl's last search.
    last_search: Mutex<Value>,
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
    let mut page = json!({ "url": "https://fire.example", "title": "Fire", "description": body["query"] });
    if body["scrapeOptions"].is_object() {
        page["markdown"] = json!(format!("# Fire\n\n{}", "word ".repeat(super::PAGE_CHARS / 4)));
    }
    *behaviour.last_search.lock().unwrap() = body;

    Json(json!({ "success": true, "data": { "web": [page] } })).into_response()
}

async fn firecrawl_scrape(Json(body): Json<Value>) -> Json<Value> {
    let chrome = body["excludeTags"]
        .as_array()
        .is_some_and(|tags| tags.contains(&json!("footer")));
    assert!(chrome, "the page's navigation and footer are left out");
    let markdown = format!(
        "[Skip to content](#main)\n# Page\n\nRead from {}",
        body["url"].as_str().unwrap_or_default()
    );

    Json(json!({ "success": true, "data": { "markdown": markdown, "metadata": { "title": "Page" } } }))
}

async fn firecrawl_map(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({ "success": true, "links": [
        { "url": "https://docs.example/guide/retries", "title": "Retries", "description": body["search"] },
        { "url": "https://docs.example/", "title": "Docs" }
    ] }))
}

async fn exa_contents(Json(body): Json<Value>) -> Json<Value> {
    let url = body["urls"][0].clone();

    Json(json!({
        "results": [{ "url": url, "title": "Exa page", "text": "Read by Exa." }],
        "statuses": [{ "id": url, "status": "success" }]
    }))
}

async fn tavily_extract(Json(body): Json<Value>) -> Json<Value> {
    let url = body["urls"][0].as_str().unwrap_or_default().to_string();
    if url.contains("broken") {
        return Json(
            json!({ "results": [], "failed_results": [{ "url": url, "error": "Failed to retrieve content" }] }),
        );
    }

    Json(json!({ "results": [{ "url": url, "raw_content": "Read by Tavily." }], "failed_results": [] }))
}

async fn tavily_map() -> Json<Value> {
    Json(json!({ "results": [
        "https://docs.example/",
        "https://docs.example/blog/launch",
        "https://docs.example/guide/retries"
    ] }))
}

async fn exa(State(behaviour): State<Arc<Behaviour>>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    behaviour.asked.lock().unwrap().push(format!("exa:{}", key(&headers)));
    assert_eq!(body["contents"]["highlights"], true);

    Json(json!({ "results": [{ "url": "https://exa.example", "title": "Exa", "highlights": ["one", "two"] }] }))
}

async fn tavily(State(behaviour): State<Arc<Behaviour>>, headers: HeaderMap) -> Response {
    let key = key(&headers);
    behaviour.asked.lock().unwrap().push(format!("tavily:{key}"));
    // Tavily's own status for a plan's usage limit.
    if *behaviour.refused_key.lock().unwrap() == key {
        let limit = StatusCode::from_u16(432).unwrap();
        return (
            limit,
            Json(json!({ "detail": { "error": "This request exceeds your plan's set usage limit." } })),
        )
            .into_response();
    }

    Json(json!({ "results": [{ "url": "https://tavily.example", "title": "Tavily", "content": "found" }] }))
        .into_response()
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
        .route("/firecrawl/search", post(firecrawl))
        .route("/exa/search", post(exa))
        .route("/tavily/search", post(tavily))
        .route("/firecrawl/scrape", post(firecrawl_scrape))
        .route("/firecrawl/map", post(firecrawl_map))
        .route("/exa/contents", post(exa_contents))
        .route("/tavily/extract", post(tavily_extract))
        .route("/tavily/map", post(tavily_map))
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

fn query(text: &str) -> Query<'_> {
    Query {
        text,
        limit: 5,
        ..Query::default()
    }
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

    let (provider, hits) = engine.web_search("s1", query("rust")).await.unwrap();
    assert_eq!(provider, "firecrawl");
    assert_eq!(
        hits,
        [Hit {
            url: "https://fire.example".into(),
            title: "Fire".into(),
            snippet: "rust".into(),
            content: None,
        }]
    );
    engine.web_search("s1", query("rust")).await.unwrap();
    assert_eq!(asked(&behaviour, "firecrawl:spent"), 1, "a 402 sets the key aside");
    assert_eq!(asked(&behaviour, "firecrawl:fresh"), 2);

    engine.add_search_key("firecrawl", "spare").unwrap();
    engine.web_search("s1", query("rust")).await.unwrap();
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

    let (provider, hits) = engine.web_search("s1", query("rust")).await.unwrap();
    assert_eq!(provider, "exa");
    assert_eq!(hits[0].snippet, "one two");
    behaviour.firecrawl_busy.store(false, Ordering::SeqCst);

    let (provider, _) = engine.web_search("s2", query("rust")).await.unwrap();
    assert_eq!(provider, "exa", "firecrawl's key still cools");
    assert_eq!(asked(&behaviour, "firecrawl:f1"), 1);

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (provider, _) = engine.web_search("s3", query("rust")).await.unwrap();
    assert_eq!(provider, "firecrawl", "back after its Retry-After");
    let (provider, _) = engine.web_search("s1", query("rust")).await.unwrap();
    assert_eq!(provider, "exa", "a session keeps the provider that last answered it");
}

#[tokio::test]
async fn tavily_answers_in_its_own_shape() {
    let behaviour = Arc::new(Behaviour::default());
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("tavily", "t1").unwrap();
    choose(&engine, "tavily", &[]);

    let (_, hits) = engine.web_search("s1", query("rust")).await.unwrap();
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
    assert!(engine.web_search("s1", query("rust")).await.is_err());
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
    let (provider, hits) = engine.web_search("s1", query("rust & wasm")).await.unwrap();
    assert_eq!(provider, "searxng");
    assert_eq!(hits[0].url, "https://searx.example");
    assert_eq!(hits[0].snippet, "rust & wasm", "the query reaches the instance intact");
}

#[tokio::test]
async fn pages_and_the_developer_index_are_asked_for_and_a_long_page_is_cut() {
    let behaviour = Arc::new(Behaviour::default());
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("firecrawl", "f1").unwrap();
    choose(&engine, "auto", &[]);

    let asked = Query {
        content: true,
        developer: true,
        ..query("tokio retry backoff")
    };
    let (_, hits) = engine.web_search("s1", asked).await.unwrap();
    let body = behaviour.last_search.lock().unwrap().clone();
    assert_eq!(body["scrapeOptions"]["formats"], json!(["markdown"]));
    assert!(
        body["scrapeOptions"]["excludeTags"]
            .as_array()
            .is_some_and(|tags| tags.contains(&json!("nav")))
    );
    assert_eq!(body["categories"], json!([{ "type": "developer" }]));
    let page = hits[0].content.as_deref().unwrap();
    assert!(page.starts_with("# Fire"));
    assert!(
        page.ends_with(&format!("(cut at {} characters)", super::PAGE_CHARS)),
        "{}",
        &page[page.len() - 60..]
    );

    engine.web_search("s1", query("tokio")).await.unwrap();
    let plain = behaviour.last_search.lock().unwrap().clone();
    assert!(plain.get("scrapeOptions").is_none() && plain.get("categories").is_none());

    engine.add_search_key("exa", "e1").unwrap();
    choose(&engine, "exa", &[]);
    let error = engine.web_search("s1", asked).await.unwrap_err();
    assert!(error.contains("developer index"), "{error}");
}

#[tokio::test]
async fn pages_and_site_maps_go_only_to_providers_that_can() {
    let behaviour = Arc::new(Behaviour::default());
    let (engine, _) = engine(&behaviour).await;
    for (provider, key) in [("exa", "e1"), ("tavily", "t1"), ("firecrawl", "f1")] {
        engine.add_search_key(provider, key).unwrap();
    }
    choose(&engine, "auto", &["exa", "tavily", "firecrawl"]);

    let (provider, page) = engine.web_scrape("s1", "https://docs.example/guide").await.unwrap();
    assert_eq!((provider.as_str(), page.text.as_str()), ("exa", "Read by Exa."));

    // Exa cannot list a site's pages, so Tavily does, ranked by the query since it orders nothing itself.
    let (provider, pages) = engine
        .web_map("s2", "https://docs.example", Some("retries"), 2)
        .await
        .unwrap();
    assert_eq!(provider, "tavily");
    let urls: Vec<&str> = pages.iter().map(|page| page.url.as_str()).collect();
    assert_eq!(urls, ["https://docs.example/guide/retries", "https://docs.example/"]);

    choose(&engine, "firecrawl", &[]);
    let (_, page) = engine.web_scrape("s6", "https://docs.example/guide").await.unwrap();
    assert!(page.text.starts_with("# Page"), "the skip link goes: {}", page.text);
    let (_, pages) = engine
        .web_map("s3", "https://docs.example", Some("retries"), 5)
        .await
        .unwrap();
    assert_eq!(pages[0].snippet, "retries", "Firecrawl is given the query to order by");

    choose(&engine, "tavily", &[]);
    let broken = engine
        .web_scrape("s4", "https://docs.example/broken")
        .await
        .unwrap_err();
    assert!(broken.contains("Failed to retrieve content"), "{broken}");

    choose(&engine, "exa", &[]);
    let error = engine.web_map("s5", "https://docs.example", None, 5).await.unwrap_err();
    assert!(error.contains("list a site's pages"), "{error}");
}

#[tokio::test]
async fn a_tavily_key_past_its_plan_limit_is_set_aside() {
    let behaviour = Arc::new(Behaviour::default());
    *behaviour.refused_key.lock().unwrap() = "t-spent".into();
    let (engine, _) = engine(&behaviour).await;
    engine.add_search_key("tavily", "t-spent").unwrap();
    engine.add_search_key("tavily", "t-fresh").unwrap();
    choose(&engine, "tavily", &[]);

    engine.web_search("s1", query("rust")).await.unwrap();
    engine.web_search("s1", query("rust")).await.unwrap();

    assert_eq!(asked(&behaviour, "tavily:t-spent"), 1, "a 432 sets the key aside");
    assert_eq!(asked(&behaviour, "tavily:t-fresh"), 2);
}

#[test]
fn a_snippet_reads_as_text_without_markdown_links_or_heading_marks() {
    let firecrawl = "## [\u{200b}](https://docs.firecrawl.dev/features/search#scraping) Search with [content](https://x.dev) scraping [1]";

    assert_eq!(super::plain(firecrawl), "Search with content scraping [1]");
}

#[test]
fn a_page_loses_the_links_a_site_puts_before_it_and_a_snippet_its_engine_tail() {
    let docs = "[Skip to main content](https://docs.rs/#main)\n\n- [Settings](https://docs.rs/settings.html)\n[Help](https://docs.rs/help.html)\n\n## [select](https://docs.rs/#) waits on branches\n\n[More](https://docs.rs/more)";
    assert_eq!(
        super::without_chrome(docs),
        "## [select](https://docs.rs/#) waits on branches\n\n[More](https://docs.rs/more)"
    );

    let links_only = "[One](https://a.example)\n[Two](https://b.example)";
    assert_eq!(
        super::without_chrome(links_only),
        links_only,
        "a page of links keeps them"
    );

    let snippet = "The error you're getting is the first one listed. Best setup for Tauri releases?More results from www.reddit.com";
    assert_eq!(
        super::plain(snippet),
        "The error you're getting is the first one listed. Best setup for Tauri releases?"
    );
}
