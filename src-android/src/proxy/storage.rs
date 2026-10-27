//! The page's saved values, kept by the app. The WebView keeps storage per origin, and the proxy's port, part of the
//! origin, changes every launch; so the page is given them back before any of its scripts run, and sends the whole
//! set here whenever it changes.

use std::collections::{BTreeMap, HashSet};
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

/// Replaces the saved values with the page's whole storage, or applies its changes since a snapshot this kept, unless
/// that page already sent a later snapshot or a newer page load has written since.
async fn keep(State(proxy): State<Proxy>, Query(snapshot): Query<Snapshot>, body: Bytes) -> Response {
    let mut storing = proxy.storing.lock().unwrap();
    if storing.is_stale(&snapshot) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let values = match snapshot.base {
        None => serde_json::from_slice::<BTreeMap<String, String>>(&body).ok(),
        Some(base) if storing.holds(&snapshot.page, base) => serde_json::from_slice::<Changes>(&body)
            .ok()
            .map(|changes| changes.onto(read(&proxy.storage))),
        Some(_) => return StatusCode::CONFLICT.into_response(),
    };
    let Some(values) = values else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match write(&proxy.storage, &values) {
        Ok(()) => {
            storing.kept(snapshot);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// What changed since a kept snapshot, sent when the whole set is too large to outlive the page.
#[derive(Deserialize)]
struct Changes {
    set: BTreeMap<String, String>,
    removed: Vec<String>,
}

impl Changes {
    fn onto(self, mut values: BTreeMap<String, String>) -> BTreeMap<String, String> {
        for key in self.removed {
            values.remove(&key);
        }
        values.extend(self.set);
        values
    }
}

/// The last snapshot written, and the page loads it replaced, whose late snapshots would undo a newer page's.
#[derive(Default)]
pub(super) struct Storing {
    newest: Option<Snapshot>,
    retired: HashSet<String>,
}

impl Storing {
    fn is_stale(&self, snapshot: &Snapshot) -> bool {
        self.retired.contains(&snapshot.page)
            || self
                .newest
                .as_ref()
                .is_some_and(|kept| kept.page == snapshot.page && kept.sequence >= snapshot.sequence)
    }

    fn holds(&self, page: &str, sequence: u64) -> bool {
        self.newest
            .as_ref()
            .is_some_and(|kept| kept.page == page && kept.sequence == sequence)
    }

    fn kept(&mut self, snapshot: Snapshot) {
        if let Some(previous) = self.newest.take().filter(|kept| kept.page != snapshot.page) {
            self.retired.insert(previous.page);
        }
        self.newest = Some(snapshot);
    }
}

/// Which page sent a snapshot, how many it had sent by then, and the kept snapshot its changes are from.
#[derive(Deserialize)]
pub(super) struct Snapshot {
    page: String,
    sequence: u64,
    base: Option<u64>,
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
