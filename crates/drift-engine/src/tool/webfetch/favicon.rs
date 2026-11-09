//! A site's icon, for the transcript to show beside a URL it fetched.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use super::{BROWSER, client, elsewhere, public};

#[derive(Clone)]
pub struct Favicon {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

const MAX_ICON: usize = 100 * 1024;
const MAX_HEAD: usize = 256 * 1024;
const MAX_CACHED: usize = 512;
const MAX_HOPS: usize = 4;
const WAIT: Duration = Duration::from_secs(10);

static CACHE: LazyLock<Mutex<HashMap<String, Option<Favicon>>>> = LazyLock::new(Default::default);

/// The icon of a public `host`, fetched once and remembered, found or not, until the cache fills.
pub async fn get(host: &str) -> Option<Favicon> {
    let host = host.to_ascii_lowercase();
    if !valid(&host) {
        return None;
    }
    if let Some(known) = CACHE.lock().ok()?.get(&host) {
        return known.clone();
    }

    let found = tokio::time::timeout(WAIT, find(&host)).await.ok().flatten();
    let mut cache = CACHE.lock().ok()?;
    if cache.len() >= MAX_CACHED {
        cache.clear();
    }
    cache.insert(host, found.clone());
    found
}

fn valid(host: &str) -> bool {
    let named = host.len() <= 253
        && host.contains('.')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    named && public(&format!("https://{host}/"))
}

/// `/favicon.ico` first, as most sites serve one; else the icon the home page links.
async fn find(host: &str) -> Option<Favicon> {
    if let Some(icon) = icon(&format!("https://{host}/favicon.ico")).await {
        return Some(icon);
    }
    let (base, page) = get_bytes(&format!("https://{host}/"), MAX_HEAD).await?;
    let href = icon_href(&String::from_utf8_lossy(&page))?;
    icon(base.join(&href).ok()?.as_str()).await
}

async fn icon(url: &str) -> Option<Favicon> {
    let (_, bytes) = get_bytes(url, MAX_ICON + 1).await?;
    (bytes.len() <= MAX_ICON).then_some(())?;
    Some(Favicon {
        mime: mime(&bytes)?,
        bytes,
    })
}

/// The body's first `limit` bytes and the URL it came from, following redirects to other public hosts.
async fn get_bytes(url: &str, limit: usize) -> Option<(reqwest::Url, Vec<u8>)> {
    let client = client(true);
    let mut url = url.to_string();
    for _ in 0..MAX_HOPS {
        let response = client.get(&url).header("user-agent", BROWSER).send().await.ok()?;
        if let Some(target) = elsewhere(&response) {
            public(&target).then_some(())?;
            url = target;
            continue;
        }
        response.status().is_success().then_some(())?;
        let from = response.url().clone();
        return Some((from, read(response, limit).await?));
    }
    None
}

async fn read(mut response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let Some(chunk) = response.chunk().await.ok()? else {
            break;
        };
        bytes.extend_from_slice(&chunk);
    }
    bytes.truncate(limit);
    Some(bytes)
}

fn mime(bytes: &[u8]) -> Option<&'static str> {
    if let Some(mime) = crate::tool::image::sniff(bytes) {
        return Some(mime);
    }
    if bytes.starts_with(&[0, 0, 1, 0]) {
        return Some("image/x-icon");
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    head.contains("<svg").then_some("image/svg+xml")
}

/// The `href` of the page's icon link, preferring a plain icon to an Apple touch icon.
fn icon_href(page: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let mut touch = None;
    let mut from = 0;
    while let Some(found) = lower[from..].find("<link") {
        let start = from + found;
        let end = lower[start..].find('>').map_or(lower.len(), |end| start + end);
        from = end;
        let tag = &page[start..end];
        let rel = attribute(tag, "rel").unwrap_or_default().to_ascii_lowercase();
        let Some(href) = attribute(tag, "href").filter(|href| !href.is_empty()) else {
            continue;
        };
        if rel.split_whitespace().any(|token| token == "icon") {
            return Some(href.to_string());
        }
        if rel.contains("apple-touch-icon") && touch.is_none() {
            touch = Some(href.to_string());
        }
    }
    touch
}

/// An attribute's value in an HTML tag, quoted or bare.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let start = from + found;
        from = start + name.len();
        let spaced = lower[..start].ends_with(char::is_whitespace);
        let rest = lower[from..].trim_start();
        if !spaced || !rest.starts_with('=') {
            continue;
        }
        let value = tag[tag.len() - rest.len() + 1..].trim_start();
        return match value.chars().next()? {
            quote @ ('"' | '\'') => value[1..].split(quote).next(),
            _ => value.split(|c: char| c.is_whitespace() || c == '>').next(),
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_icon_is_found_before_a_touch_icon() {
        let page = r#"<head><link rel="apple-touch-icon" href="/touch.png">
            <link hreflang="en" href='/static/fav.svg' rel="shortcut icon"></head>"#;
        assert_eq!(icon_href(page).as_deref(), Some("/static/fav.svg"));
        let touch = r#"<link rel=apple-touch-icon href=/touch.png><link rel="stylesheet" href="a.css">"#;
        assert_eq!(icon_href(touch).as_deref(), Some("/touch.png"));
        assert_eq!(icon_href("<link rel=\"stylesheet\" href=\"a.css\">"), None);
    }

    #[test]
    fn only_public_names_are_asked_for() {
        assert!(valid("docs.rs"));
        assert!(!valid("localhost"));
        assert!(!valid("printer.local"));
        assert!(!valid("192.168.1.1"));
        assert!(!valid("a.com/../b"));
        assert!(!valid("user@a.com"));
    }

    #[test]
    fn icons_are_known_by_their_bytes() {
        assert_eq!(mime(&[0, 0, 1, 0, 1, 0]), Some("image/x-icon"));
        assert_eq!(mime(b"<?xml version=\"1.0\"?><svg xmlns=\"\">"), Some("image/svg+xml"));
        assert_eq!(mime(b"<!doctype html><html>"), None);
    }
}
