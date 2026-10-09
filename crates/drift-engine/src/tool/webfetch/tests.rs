use super::super::tests::Sandbox;
use super::*;
use axum::Router;
use axum::routing::get;

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
