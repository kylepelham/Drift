//! The page's saved values, kept by the app. The WebView keeps storage per origin, and the proxy's port, part of the
//! origin, changes every launch; so the page is given them back before any of its scripts run, and sends the whole
//! set here whenever it changes.

use std::collections::BTreeMap;
use std::path::Path;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde::Deserialize;

use super::Proxy;

/// Far beyond what settings, drafts and the selection take; a page past it keeps its values for this launch only.
const MAX_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn routes() -> Router<Proxy> {
    Router::new()
        .route("/storage.js", get(restore))
        .route("/storage", put(keep).layer(DefaultBodyLimit::max(MAX_BYTES)))
}

/// The page with its saved values restored first, ahead of its own scripts that read them.
pub(super) fn with_restore(page: &str) -> String {
    page.replacen("<head>", r#"<head><script src="/app/api/storage.js"></script>"#, 1)
}

/// A script that puts the saved values back into this launch's storage.
async fn restore(State(proxy): State<Proxy>) -> Response {
    let saved = read(&proxy.storage);
    let values = serde_json::to_string(&saved).unwrap_or_else(|_| "{}".into());
    let script = format!(
        "(function(){{var saved={values};for(var key in saved)try{{localStorage.setItem(key,saved[key])}}catch(e){{}}}})();"
    );

    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        script,
    )
        .into_response()
}

/// Replaces the saved values with the page's whole storage, unless that page already sent a later snapshot.
async fn keep(State(proxy): State<Proxy>, Query(snapshot): Query<Snapshot>, body: Bytes) -> Response {
    let Ok(values) = serde_json::from_slice::<BTreeMap<String, String>>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let mut newest = proxy.storing.lock().unwrap();
    if newest
        .as_ref()
        .is_some_and(|kept| kept.page == snapshot.page && kept.sequence >= snapshot.sequence)
    {
        return StatusCode::NO_CONTENT.into_response();
    }
    match write(&proxy.storage, &values) {
        Ok(()) => {
            *newest = Some(snapshot);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Which page sent a snapshot and how many it had sent by then.
#[derive(Deserialize)]
pub(super) struct Snapshot {
    page: String,
    sequence: u64,
}

fn read(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(path: &Path, values: &BTreeMap<String, String>) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let staged = path.with_extension("json.new");
    std::fs::write(&staged, serde_json::to_vec(values).map_err(std::io::Error::other)?)?;

    std::fs::rename(staged, path)
}
