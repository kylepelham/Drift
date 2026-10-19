use std::time::Duration;

use serde_json::{Value, json};

use super::ToolMetadata;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, image, required_str};
use crate::llm::ToolSpec;

const TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const MAX_BYTES: usize = 5 * 1024 * 1024;
/// An HTML page with less visible text than this is taken for one that builds itself with JavaScript.
const THIN_PAGE_CHARS: usize = 200;

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

    /// Fetches the page itself first; a page that will not load, or loads nearly empty, is read through a web search
    /// provider instead when one has a key.
    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let url = required_str(&input, "url")?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ToolError("url must start with http:// or https://".into()));
            }
            let format = input["format"].as_str().unwrap_or("markdown");

            match direct(ctx, url, format).await? {
                Fetched::Done(output) => Ok(output),
                Fetched::Thin(output) => Ok(match through_provider(ctx, url).await {
                    Some(Ok(rendered)) => rendered,
                    _ => output,
                }),
                Fetched::Blocked(error) => match through_provider(ctx, url).await {
                    Some(Ok(rendered)) => Ok(rendered),
                    Some(Err(why)) => Err(ToolError(format!("{}; {why}", error.0))),
                    None => Err(error),
                },
            }
        })
    }
}

/// What fetching the page itself got.
enum Fetched {
    /// A page or file to return as it is.
    Done(Output),
    /// An HTML page with almost no text, as one that builds itself with JavaScript sends.
    Thin(Output),
    /// Refused or unreachable in a way a provider may get past: blocked, rate limited, or no connection.
    Blocked(ToolError),
}

/// The page as this machine fetches it. A missing page, one too large, or an abort is an error no provider helps with.
async fn direct(ctx: &Context, url: &str, format: &str) -> Result<Fetched, ToolError> {
    let response = match respond(ctx, url).await {
        Ok(response) => response,
        Err(error) if ctx.abort.is_cancelled() => return Err(error),
        Err(error) => return Ok(Fetched::Blocked(error)),
    };
    if let Some(target) = elsewhere(&response) {
        return Ok(Fetched::Done(redirected(url, target)));
    }
    let status = response.status();
    if !status.is_success() {
        let error = ToolError(format!("{url} answered {status}"));
        // A page that is not there is not there for a provider either.
        let gone = matches!(status.as_u16(), 404 | 410);
        return if gone { Err(error) } else { Ok(Fetched::Blocked(error)) };
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
        return fetched_file(url, mime, &bytes).map(Fetched::Done);
    }
    if bytes.len() > MAX_BYTES {
        return Err(ToolError(format!("{url} is {} bytes; too large to fetch", bytes.len())));
    }

    let body = String::from_utf8_lossy(&bytes);
    let thin = content_type.contains("text/html") && visible_chars(&body) < THIN_PAGE_CHARS;
    let output = Output {
        title: url.into(),
        output: page_text(body, &content_type, format).trim().into(),
        metadata: ToolMetadata {
            content_type: Some(content_type),
            bytes: Some(bytes.len()),
            ..Default::default()
        },
    };

    Ok(if thin {
        Fetched::Thin(output)
    } else {
        Fetched::Done(output)
    })
}

fn visible_chars(html: &str) -> usize {
    strip_tags(html).chars().filter(|c| !c.is_whitespace()).count()
}

/// The page read through a web search provider, as markdown; `None` when no provider can read pages or the URL
/// must not leave this machine.
async fn through_provider(ctx: &Context, url: &str) -> Option<Result<Output, String>> {
    // Tests serve the site and the provider both on loopback, which a real fallback never sends anywhere.
    let allowed = public(url) || cfg!(test);
    if !allowed || !ctx.engine.web_scrape_available() {
        return None;
    }
    let read = tokio::select! {
        read = ctx.engine.web_scrape(&ctx.session_id, url) => read,
        () = ctx.abort.cancelled() => return Some(Err("aborted".into())),
    };

    Some(match read {
        Ok((provider, page)) => {
            let name = crate::websearch::provider_name(&provider);
            Ok(Output {
                title: url.into(),
                output: format!("(Read through {name}, as it did not load directly.)\n\n{}", page.text),
                metadata: ToolMetadata {
                    provider: Some(provider),
                    content_type: Some("text/markdown".into()),
                    bytes: Some(page.text.len()),
                    ..Default::default()
                },
            })
        }
        Err(why) => Err(format!("reading it through a web search provider failed too: {why}")),
    })
}

/// Whether `url` may go to a third party: a public host by name or address, with no credentials in it.
fn public(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return public_address(address);
    }

    let local = [".local", ".localhost", ".internal", ".lan", ".home.arpa"];
    host.contains('.') && !local.iter().any(|suffix| host.ends_with(suffix))
}

fn public_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            let shared = v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64;
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || shared)
        }
        std::net::IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            let local = (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80;
            let mapped = v6
                .to_ipv4_mapped()
                .is_none_or(|v4| public_address(std::net::IpAddr::V4(v4)));
            !(v6.is_loopback() || v6.is_unspecified() || local) && mapped
        }
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
