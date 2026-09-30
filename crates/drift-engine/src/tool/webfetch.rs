use std::time::Duration;

use serde_json::{json, Value};

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BYTES: usize = 5 * 1024 * 1024;
const MAX_CHARS: usize = 100_000;

pub struct WebFetch;

impl Tool for WebFetch {
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
        Some(Ask::new("webfetch", url, format!("Fetch {url}")))
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let url = required_str(&input, "url")?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ToolError("url must start with http:// or https://".into()));
            }
            let format = input["format"].as_str().unwrap_or("markdown");
            let request = ctx
                .engine
                .http
                .get(url)
                .timeout(TIMEOUT)
                .header("user-agent", "Drift/1.0 (+https://driftagent.dev)")
                .header("accept", "text/html, text/markdown, text/plain, application/json;q=0.9, */*;q=0.5");
            let response = tokio::select! {
                response = request.send() => response.map_err(|e| ToolError(format!("request failed: {e}")))?,
                () = ctx.abort.cancelled() => return Err(ToolError("aborted".into())),
            };
            let status = response.status();
            if !status.is_success() {
                return Err(ToolError(format!("{url} answered {status}")));
            }
            let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let bytes = response.bytes().await.map_err(|e| ToolError(format!("read failed: {e}")))?;
            if bytes.len() > MAX_BYTES {
                return Err(ToolError(format!("{url} is {} bytes; too large to fetch", bytes.len())));
            }
            let body = String::from_utf8_lossy(&bytes);
            let text = if content_type.contains("text/html") && format != "html" {
                if format == "text" { strip_tags(&body) } else { markdown(&body) }
            } else {
                body.into_owned()
            };
            Ok(Output { title: url.into(), output: clip(text.trim()), metadata: json!({ "contentType": content_type, "bytes": bytes.len() }) })
        })
    }
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
            None if tag.starts_with("br") || tag.starts_with("/p") || tag.starts_with("/div") || tag.starts_with("/h") => out.push('\n'),
            _ => {}
        }
        rest = &rest[start + end + 1..];
    }
    if skip_until.is_none() {
        out.push_str(rest);
    }
    out.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_CHARS {
        return text.into();
    }
    format!("{}\n\n(truncated at {MAX_CHARS} characters)", text.chars().take(MAX_CHARS).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;
    use axum::routing::get;
    use axum::Router;

    async fn page() -> axum::response::Html<&'static str> {
        axum::response::Html("<html><head><style>x{}</style><script>bad()</script></head><body><h1>Title</h1><p>Hello <b>world</b></p></body></html>")
    }

    async fn serve() -> String {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, Router::new().route("/page", get(page))).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn html_becomes_markdown_or_text() {
        let sandbox = Sandbox::new("webfetch");
        let url = serve().await;
        let md = WebFetch.run(&sandbox.ctx, json!({ "url": format!("{url}/page") })).await.unwrap();
        assert!(md.output.contains("# Title"), "{}", md.output);
        assert!(md.output.contains("**world**"));
        assert!(!md.output.contains("bad()"));
        let text = WebFetch.run(&sandbox.ctx, json!({ "url": format!("{url}/page"), "format": "text" })).await.unwrap();
        assert_eq!(text.output, "Title\nHello world");
        let missing = WebFetch.run(&sandbox.ctx, json!({ "url": format!("{url}/nope") })).await.unwrap_err();
        assert!(missing.0.contains("404"));
        assert!(WebFetch.run(&sandbox.ctx, json!({ "url": "ftp://x" })).await.is_err());
    }
}
