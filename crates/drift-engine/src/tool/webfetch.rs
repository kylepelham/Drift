use std::time::Duration;

use serde_json::{Value, json};

use super::ToolMetadata;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, image, required_str};
use crate::llm::ToolSpec;

const TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const MAX_BYTES: usize = 5 * 1024 * 1024;

pub struct WebFetch;

impl Tool for WebFetch {
    fn permissions(&self) -> &'static [&'static str] {
        &["webfetch"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "webfetch".into(),
            description: include_str!("prompts/webfetch.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Absolute http or https URL." },
                    "format": { "type": "string", "enum": ["markdown", "text", "html"], "description": "How to return HTML pages. Default markdown." }
                },
                "required": ["url"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, input: &Value) -> Option<Ask> {
        let url = input["url"].as_str()?;
        // As opencode: fetching runs without asking; a rule can still ask or deny by URL.
        Some(Ask::new("webfetch", url, format!("Fetch {url}")).allow_by_default())
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let url = required_str(&input, "url")?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ToolError("url must start with http:// or https://".into()));
            }
            let format = input["format"].as_str().unwrap_or("markdown");
            let mut response = fetch(ctx, url, BROWSER).await?;
            // Cloudflare challenges a browser agent whose TLS does not look like a browser's; an honest one often passes.
            if response.status() == reqwest::StatusCode::FORBIDDEN
                && response
                    .headers()
                    .get("cf-mitigated")
                    .is_some_and(|value| value == "challenge")
            {
                response = fetch(ctx, url, HONEST).await?;
            }
            if let Some(target) = elsewhere(&response) {
                let output = format!(
                    "{url} redirects to {target}, outside the site that was approved (another host or port, or down to plain http), which was not fetched. Fetch {target} to follow it."
                );
                return Ok(Output {
                    title: url.into(),
                    output,
                    metadata: ToolMetadata {
                        redirect: Some(target),
                        ..Default::default()
                    },
                });
            }
            let status = response.status();
            if !status.is_success() {
                return Err(ToolError(format!("{url} answered {status}")));
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let bytes = tokio::select! {
                read = read_capped(response, url) => read?,
                () = ctx.abort.cancelled() => return Err(ToolError("aborted".into())),
            };
            // An image or PDF, known by its bytes whatever the server calls it, comes back to look at.
            if let Some(mime) = image::sniff(&bytes).or(image::is_pdf(&bytes).then_some(image::PDF)) {
                return fetched_file(url, mime, &bytes);
            }
            if bytes.len() > MAX_BYTES {
                return Err(ToolError(format!("{url} is {} bytes; too large to fetch", bytes.len())));
            }
            let body = String::from_utf8_lossy(&bytes);
            let text = if content_type.contains("text/html") && format != "html" {
                if format == "text" {
                    strip_tags(&body)
                } else {
                    markdown(&body)
                }
            } else {
                body.into_owned()
            };
            Ok(Output {
                title: url.into(),
                output: text.trim().into(),
                metadata: ToolMetadata {
                    content_type: Some(content_type),
                    bytes: Some(bytes.len()),
                    ..Default::default()
                },
            })
        })
    }
}

/// Pages are asked for as a browser would; sites that refuse unknown agents are the common case.
const BROWSER: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const HONEST: &str = "Drift/1.0 (+https://driftagent.dev)";
const MAX_REDIRECTS: usize = 10;

/// Follows redirects only within the URL's own origin: the user approved that URL, not wherever it points.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt
            .previous()
            .first()
            .is_some_and(|first| same_origin(first, attempt.url()));
        match attempt.previous().len() {
            hops if hops > MAX_REDIRECTS => attempt.error("too many redirects"),
            _ if same => attempt.follow(),
            _ => attempt.stop(),
        }
    });
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .redirect(policy)
                .build()
                .unwrap_or_default()
        })
        .clone()
}

/// The same host, on the same scheme and port, or moved from http to https (the usual upgrade).
fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    let upgrade = a.scheme() == "http" && b.scheme() == "https" && a.port().is_none() && b.port().is_none();
    let same = a.scheme() == b.scheme() && a.port_or_known_default() == b.port_or_known_default();
    a.host_str() == b.host_str() && (same || upgrade)
}

async fn fetch(ctx: &Context, url: &str, agent: &str) -> Result<reqwest::Response, ToolError> {
    let request = client()
        .get(url)
        .timeout(TIMEOUT)
        .header("user-agent", agent)
        .header(
            "accept",
            "text/html, text/markdown, text/plain, application/json;q=0.9, */*;q=0.5",
        )
        .header("accept-language", "en-US,en;q=0.9");
    tokio::select! {
        response = request.send() => response.map_err(|e| ToolError(format!("request failed: {e}"))),
        () = ctx.abort.cancelled() => Err(ToolError("aborted".into())),
    }
}

/// Where a redirect the client would not follow points, resolved against the page that sent it.
fn elsewhere(response: &reqwest::Response) -> Option<String> {
    if !response.status().is_redirection() {
        return None;
    }
    let location = response.headers().get("location")?.to_str().ok()?;
    response.url().join(location).ok().map(String::from)
}

/// The body, read a chunk at a time and given up on once it passes the largest size anything accepts
/// (a PDF's), so a huge or endless response never fills memory.
async fn read_capped(mut response: reqwest::Response, url: &str) -> Result<Vec<u8>, ToolError> {
    let cap = MAX_BYTES.max(image::MAX_PDF_BYTES);
    let too_large = |size: u64| {
        ToolError(format!(
            "{url} is over {} MB ({size} bytes or more); too large to fetch",
            cap / 1024 / 1024
        ))
    };
    if let Some(size) = response.content_length().filter(|size| *size > cap as u64) {
        return Err(too_large(size));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ToolError(format!("read failed: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > cap {
            return Err(too_large(bytes.len() as u64));
        }
    }
    Ok(bytes)
}

fn fetched_file(url: &str, mime: &str, bytes: &[u8]) -> Result<Output, ToolError> {
    let limit = if mime == image::PDF {
        image::MAX_PDF_BYTES
    } else {
        image::MAX_SOURCE_BYTES
    };
    if bytes.len() > limit {
        return Err(ToolError(format!(
            "{url} is {mime} of {} bytes; too large to look at (the limit is {} MB)",
            bytes.len(),
            limit / 1024 / 1024
        )));
    }
    let file = image::Image::from_bytes(mime, bytes);
    Ok(Output {
        title: url.into(),
        output: format!(
            "{url} is {mime} ({} KB); it follows this result.",
            bytes.len().div_ceil(1024)
        ),
        metadata: ToolMetadata {
            content_type: Some(mime.into()),
            bytes: Some(bytes.len()),
            images: Some(image::metadata(&[file])),
            ..Default::default()
        },
    })
}

fn markdown(html: &str) -> String {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "nav", "footer", "svg"])
        .build()
        .convert(html)
        .unwrap_or_else(|_| strip_tags(html))
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut skip_until: Option<&str> = None;
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        if skip_until.is_none() {
            out.push_str(&rest[..start]);
        }
        let Some(end) = rest[start..].find('>') else { break };
        let tag = rest[start + 1..start + end].trim().to_ascii_lowercase();
        match skip_until {
            Some(closing) if tag == closing => skip_until = None,
            None if tag.starts_with("script") => skip_until = Some("/script"),
            None if tag.starts_with("style") => skip_until = Some("/style"),
            None if tag.starts_with("br")
                || tag.starts_with("/p")
                || tag.starts_with("/div")
                || tag.starts_with("/h") =>
            {
                out.push('\n')
            }
            _ => {}
        }
        rest = &rest[start + end + 1..];
    }
    if skip_until.is_none() {
        out.push_str(rest);
    }
    out.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
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
}
