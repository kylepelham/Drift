use super::super::tests::Sandbox;
use super::*;
use axum::Router;
use axum::routing::get;
use std::sync::Arc;

async fn page() -> axum::response::Html<&'static str> {
    axum::response::Html(
        "<html><head><style>x{}</style><script>bad()</script></head><body><h1>Title</h1><p>Hello <b>world</b></p></body></html>",
    )
}

async fn serve() -> String {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/page", get(page))
        .route(
            "/shot",
            get(|| async {
                (
                    [("content-type", "application/octet-stream")],
                    b"\x89PNG\r\n\x1a\nrest".to_vec(),
                )
            }),
        )
        .route(
            "/doc",
            get(|| async { ([("content-type", "application/pdf")], b"%PDF-1.7\n...".to_vec()) }),
        )
        .route("/hop", get(|| async { axum::response::Redirect::temporary("/page") }))
        .route(
            "/challenged",
            get(|headers: axum::http::HeaderMap| async move {
                let honest = headers
                    .get("user-agent")
                    .is_some_and(|agent| agent.to_str().unwrap_or_default().starts_with("Drift/"));
                if honest {
                    (axum::http::StatusCode::OK, [("cf-mitigated", "")], "<p>passed</p>")
                } else {
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        [("cf-mitigated", "challenge")],
                        "challenge",
                    )
                }
            }),
        )
        .route(
            "/endless",
            get(|| async {
                let chunk = bytes::Bytes::from(vec![b'x'; 64 * 1024]);
                axum::body::Body::from_stream(futures_util::stream::repeat_with(move || {
                    Ok::<_, std::io::Error>(chunk.clone())
                }))
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn an_image_or_pdf_comes_back_to_look_at_not_as_text() {
    let sandbox = Sandbox::new("webfetch-files");
    let url = serve().await;
    for (path, mime) in [("shot", "image/png"), ("doc", "application/pdf")] {
        let out = WebFetch
            .run(&sandbox.ctx, json!({ "url": format!("{url}/{path}") }))
            .await
            .unwrap();
        assert_eq!(image::returned(&out.metadata)[0].mime, mime, "{path}");
        assert!(out.output.contains("it follows this result"));
    }
}

#[tokio::test]
async fn html_becomes_markdown_or_text() {
    let sandbox = Sandbox::new("webfetch");
    let url = serve().await;
    let md = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{url}/page") }))
        .await
        .unwrap();
    assert!(md.output.contains("# Title"), "{}", md.output);
    assert!(md.output.contains("**world**"));
    assert!(!md.output.contains("bad()"));
    let text = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{url}/page"), "format": "text" }))
        .await
        .unwrap();
    assert_eq!(text.output, "Title\nHello world");
    let missing = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{url}/nope") }))
        .await
        .unwrap_err();
    assert!(missing.0.contains("404"));
    assert!(WebFetch.run(&sandbox.ctx, json!({ "url": "ftp://x" })).await.is_err());
}

#[test]
fn an_upgrade_to_https_on_the_same_host_is_followed_and_nothing_else_off_origin() {
    let url = |text: &str| reqwest::Url::parse(text).unwrap();
    assert!(same_origin(&url("http://site.dev/a"), &url("https://site.dev/b")));
    assert!(same_origin(&url("https://site.dev/a"), &url("https://site.dev/b")));
    assert!(
        !same_origin(&url("https://site.dev/a"), &url("http://site.dev/b")),
        "never down to http"
    );
    assert!(!same_origin(&url("https://site.dev/a"), &url("https://other.dev/a")));
    assert!(!same_origin(
        &url("https://site.dev/a"),
        &url("https://site.dev:8443/a")
    ));
}

#[tokio::test]
async fn redirects_are_followed_only_within_the_origin_and_a_challenge_is_retried_honestly() {
    let sandbox = Sandbox::new("webfetch-redirect");
    let url = serve().await;
    let followed = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{url}/hop") }))
        .await
        .unwrap();
    assert!(
        followed.output.contains("# Title"),
        "same origin is followed: {}",
        followed.output
    );
    let away = url.replace("127.0.0.1", "localhost");
    let app = Router::new().route(
        "/away",
        get(move || {
            let target = format!("{away}/page");
            async move { axum::response::Redirect::temporary(&target) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let stopped = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{origin}/away") }))
        .await
        .unwrap();
    assert!(
        stopped.output.contains("outside the site that was approved") && !stopped.output.contains("# Title"),
        "{}",
        stopped.output
    );
    assert!(
        stopped
            .metadata
            .redirect
            .as_deref()
            .unwrap()
            .starts_with("http://localhost:")
    );
    let passed = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{url}/challenged") }))
        .await
        .unwrap();
    assert!(passed.output.contains("passed"), "{}", passed.output);
}

#[tokio::test]
async fn a_body_past_the_cap_is_given_up_on_while_it_streams() {
    let sandbox = Sandbox::new("webfetch-endless");
    let url = serve().await;
    let refused = WebFetch
        .run(&sandbox.ctx, json!({ "url": format!("{url}/endless") }))
        .await
        .unwrap_err();
    assert!(refused.0.contains("too large to fetch"), "{}", refused.0);
}

/// A site that refuses plain fetches, one that builds itself with JavaScript, and Firecrawl's scrape standing in.
async fn serve_with_provider(asked: Arc<std::sync::Mutex<Vec<String>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shell = "<html><body><div id=\"root\"></div><noscript>Enable JavaScript.</noscript><script src=\"/app.js\"></script></body></html>";
    let app = Router::new()
        .route(
            "/blocked",
            get(|| async { (axum::http::StatusCode::FORBIDDEN, "denied") }),
        )
        .route("/shell", get(move || async move { axum::response::Html(shell) }))
        .route(
            "/firecrawl/scrape",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let asked = asked.clone();
                async move {
                    asked
                        .lock()
                        .unwrap()
                        .push(body["url"].as_str().unwrap_or_default().to_string());
                    axum::Json(json!({
                        "success": true,
                        "data": { "markdown": "# Rendered\n\nThe pricing table.", "metadata": { "title": "Pricing" } }
                    }))
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    url
}

#[tokio::test]
async fn a_page_that_will_not_load_or_loads_empty_is_read_through_a_provider() {
    let sandbox = Sandbox::new("webfetch-provider");
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let url = serve_with_provider(asked.clone()).await;
    let fetch = |path: &str| WebFetch.run(&sandbox.ctx, json!({ "url": format!("{url}/{path}") }));

    // Without a provider key, the plain fetch's own answer stands.
    assert!(fetch("blocked").await.unwrap_err().0.contains("403"));
    assert!(
        fetch("shell").await.unwrap().output.is_empty(),
        "a JavaScript shell reads as nothing"
    );
    assert!(asked.lock().unwrap().is_empty());

    sandbox.ctx.engine.add_search_key("firecrawl", "fc-spare").unwrap();
    *sandbox.ctx.engine.search_routing.base.lock().unwrap() = Some(url.clone());
    for path in ["blocked", "shell"] {
        let read = fetch(path).await.unwrap();
        assert!(
            read.output.starts_with("(Read through Firecrawl"),
            "{path}: {}",
            read.output
        );
        assert!(read.output.contains("The pricing table."));
        assert_eq!(read.metadata.provider.as_deref(), Some("firecrawl"));
    }
    assert_eq!(asked.lock().unwrap().len(), 2);

    let missing = fetch("nowhere").await.unwrap_err();
    assert!(missing.0.contains("404"), "{}", missing.0);
    assert_eq!(
        asked.lock().unwrap().len(),
        2,
        "a page that is not there is not asked of a provider"
    );
}

#[test]
fn only_a_public_address_without_credentials_goes_to_a_provider() {
    for url in [
        "https://docs.rs/tokio",
        "https://93.184.215.14/page",
        "http://[2606:4700::6810:84e5]/",
    ] {
        assert!(super::public(url), "{url}");
    }
    for url in [
        "http://localhost:3000/",
        "http://127.0.0.1/admin",
        "http://10.0.0.5/",
        "http://192.168.1.20:8080/",
        "http://172.20.0.1/",
        "http://100.64.0.9/",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]/",
        "http://[fd00::1]/",
        "http://[::ffff:192.168.0.1]/",
        "http://nas/",
        "http://printer.local/",
        "http://build.internal/",
        "https://user:hunter2@example.com/",
    ] {
        assert!(!super::public(url), "{url}");
    }
}

#[test]
fn a_public_url_fetches_unasked_and_this_machine_or_its_network_asks() {
    let sandbox = Sandbox::new("webfetch-ask");
    let ask = |url: &str| WebFetch.ask(&sandbox.ctx, &json!({ "url": url })).unwrap();
    assert!(ask("https://docs.rs/tokio").default_allow);
    for url in [
        "http://169.254.169.254/latest/meta-data/",
        "http://127.0.0.1:8080/",
        "http://localhost/",
    ] {
        assert!(!ask(url).default_allow, "{url}");
    }
}

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_stays_on_this_machine() {
    assert!(!super::resolves_public("http://localhost:3000/").await);
    assert!(super::resolves_public("https://drift-unresolvable.invalid/").await);
}
