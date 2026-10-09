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

            let response = respond(ctx, url).await?;
            if let Some(target) = elsewhere(&response) {
                return Ok(redirected(url, target));
            }
            let status = response.status();
            if !status.is_success() {
                return Err(ToolError(format!("{url} answered {status}")));
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
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
            let text = page_text(body, &content_type, format);
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

/// The page as a browser asks for it, asked again honestly when Cloudflare challenges the browser agent.
async fn respond(ctx: &Context, url: &str) -> Result<reqwest::Response, ToolError> {
    let response = fetch(ctx, url, BROWSER).await?;
    // Cloudflare challenges a browser agent whose TLS does not look like a browser's; an honest one often passes.
    let challenged = response.status() == reqwest::StatusCode::FORBIDDEN
        && response
            .headers()
            .get("cf-mitigated")
            .is_some_and(|value| value == "challenge");
    if challenged {
        return fetch(ctx, url, HONEST).await;
    }
    Ok(response)
}

/// The answer for a redirect off the approved origin: where it points, unfetched.
fn redirected(url: &str, target: String) -> Output {
    let output = format!(
        "{url} redirects to {target}, outside the site that was approved (another host or port, or down to plain http), which was not fetched. Fetch {target} to follow it."
    );
    Output {
        title: url.into(),
        output,
        metadata: ToolMetadata {
            redirect: Some(target),
            ..Default::default()
        },
    }
}

/// HTML as markdown or plain text unless raw HTML was asked for; any other body as it came.
fn page_text(body: std::borrow::Cow<'_, str>, content_type: &str, format: &str) -> String {
    if !content_type.contains("text/html") || format == "html" {
        return body.into_owned();
    }
    if format == "text" {
        return strip_tags(&body);
    }
    markdown(&body)
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
fn same_origin(from: &reqwest::Url, to: &reqwest::Url) -> bool {
    let upgrade = from.scheme() == "http" && to.scheme() == "https" && from.port().is_none() && to.port().is_none();
    let same = from.scheme() == to.scheme() && from.port_or_known_default() == to.port_or_known_default();
    from.host_str() == to.host_str() && (same || upgrade)
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
        response = request.send() => response.map_err(|error| ToolError(format!("request failed: {error}"))),
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
        .map_err(|error| ToolError(format!("read failed: {error}")))?
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
                out.push('\n');
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
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
