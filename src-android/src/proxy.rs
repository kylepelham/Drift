//! The phone's side of a PC: a server on 127.0.0.1 that the WebView treats as the PC's gateway. It serves the
//! bundled UI, answers the app's own computers page, and passes everything else to the chosen PC over TLS that
//! trusts only that PC, adding this phone's session. Other apps on the phone cannot use it: every request needs
//! the cookie only the WebView was given, with this launch's secret.

mod api;
mod forward;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use rust_embed::RustEmbed;

use crate::computers::{Computer, Computers};
use crate::native::Native;
use crate::pc::{self, Pc, PcError};

const COOKIE: &str = "drift_app";

#[derive(RustEmbed)]
#[folder = "../dist"]
struct Dist;

/// The PC requests go to now, and the session they carry.
#[derive(Clone)]
pub(crate) struct Route {
    pub id: String,
    pub pc: Pc,
    pub token: String,
}

pub(crate) struct Shared {
    pub computers: Mutex<Computers>,
    pub route: Mutex<Option<Route>>,
    pub native: Arc<Native>,
    /// Held while looking for the chosen PC again, so a burst of requests looks once.
    reaching: tokio::sync::Mutex<()>,
    secret: String,
    port: u16,
    /// The page's saved values (`storage`), and the last snapshot written, held while writing so writes go one at a time.
    storage: PathBuf,
    storing: Mutex<storage::Storing>,
}

pub(crate) type Proxy = Arc<Shared>;

/// Why a request could not be passed on.
pub(crate) enum Unrouted {
    NoneChosen,
    Unreachable,
}

impl Shared {
    pub(crate) fn new(computers: Computers, native: Arc<Native>, secret: String, port: u16, storage: PathBuf) -> Proxy {
        Arc::new(Self {
            computers: Mutex::new(computers),
            route: Mutex::new(None),
            native,
            reaching: tokio::sync::Mutex::new(()),
            secret,
            port,
            storage,
            storing: Mutex::default(),
        })
    }

    /// Where requests go now; after a dropped connection the chosen PC is looked for again, so an open page recovers.
    pub(crate) async fn destination(&self) -> Result<Route, Unrouted> {
        if let Some(route) = self.route.lock().unwrap().clone() {
            return Ok(route);
        }

        let _one_search = self.reaching.lock().await;
        if let Some(route) = self.route.lock().unwrap().clone() {
            return Ok(route);
        }
        let active = self.computers.lock().unwrap().active().cloned();
        let computer = active.ok_or(Unrouted::NoneChosen)?;
        // A PC reached but not saved still routes; only no route at all is unreachable.
        let _ = self.connect(&computer).await;

        self.route.lock().unwrap().clone().ok_or(Unrouted::Unreachable)
    }

    /// Reaches a linked PC, makes it the one requests go to, and remembers it for the next launch.
    pub(crate) async fn connect(&self, computer: &Computer) -> Result<(), PcError> {
        let (pc, reach) = pc::reach(computer).await?;
        let mut computers = self.computers.lock().unwrap();
        let refreshed = computers.refresh(&computer.id, &pc.endpoint, reach);
        let activated = computers.set_active(Some(&computer.id));
        drop(computers);

        *self.route.lock().unwrap() = Some(Route {
            id: computer.id.clone(),
            pc,
            token: computer.token.clone(),
        });
        refreshed.and(activated).map_err(PcError::Unsaved)
    }

    /// Saves a PC just linked and connects to it at the address it answered on.
    pub(crate) fn adopt(&self, computer: Computer, pc: Pc) -> Result<(), PcError> {
        let mut computers = self.computers.lock().unwrap();
        let saved = computers.upsert(computer.clone());
        let activated = computers.set_active(Some(&computer.id));
        drop(computers);

        *self.route.lock().unwrap() = Some(Route {
            id: computer.id,
            pc,
            token: computer.token,
        });
        saved.and(activated).map_err(PcError::Unsaved)
    }

    /// How the PC lists this phone: the name the user gave it, read on Android's own thread.
    pub(crate) async fn device_name(&self) -> String {
        let native = self.native.clone();
        tokio::task::spawn_blocking(move || native.device_name())
            .await
            .unwrap_or_else(|_| "Android".into())
    }
}

pub(crate) fn router(proxy: Proxy) -> Router {
    Router::new()
        .route("/app/enter", get(enter))
        .route("/app", get(main_page))
        .route("/app/computers", get(computers_page))
        .nest("/app/api", api::routes().merge(storage::routes()))
        .route("/auth/me", any(forward::forward))
        .route("/auth/logout", any(forward::forward))
        .route("/engine/{*rest}", any(forward::forward))
        .route("/api/{*rest}", any(forward::forward))
        .fallback(asset)
        .layer(middleware::from_fn_with_state(proxy.clone(), gate))
        .with_state(proxy)
}

pub(crate) async fn serve(listener: tokio::net::TcpListener, proxy: Proxy) {
    let _ = axum::serve(listener, router(proxy)).await;
}

/// Only this launch's WebView: the right host (no DNS rebinding) and the cookie `/app/enter` gave it.
async fn gate(State(proxy): State<Proxy>, request: Request, next: Next) -> Response {
    let expected = format!("127.0.0.1:{}", proxy.port);
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host != Some(expected.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.uri().path() == "/app/enter" || holds_secret(request.headers(), &proxy.secret) {
        return next.run(request).await;
    }

    StatusCode::UNAUTHORIZED.into_response()
}

fn holds_secret(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .any(|(name, value)| name == COOKIE && same(value.as_bytes(), secret.as_bytes()))
}

/// Compares in time that does not depend on where the inputs differ.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// The WebView's first request: trades the launch secret in the URL for the cookie every later request carries.
async fn enter(State(proxy): State<Proxy>, Query(query): Query<HashMap<String, String>>) -> Response {
    let supplied = query.get("secret").map(String::as_bytes).unwrap_or_default();
    if !same(supplied, proxy.secret.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let cookie = format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/", proxy.secret);
    let mut response = Redirect::to("/app").into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }

    response
}

/// Drift itself, connected to the chosen PC; the computers page when there is none or it cannot be reached.
async fn main_page(State(proxy): State<Proxy>) -> Response {
    let active = proxy.computers.lock().unwrap().active().cloned();
    let Some(computer) = active else {
        return Redirect::to("/app/computers").into_response();
    };

    let routed = proxy
        .route
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|route| route.id == computer.id);
    if !routed && let Err(problem) = proxy.connect(&computer).await {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("problem", &problem.to_string())
            .finish();
        return Redirect::to(&format!("/app/computers?{query}")).into_response();
    }

    let Some(page) = Dist::get("index.html") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let page = storage::with_restore(&String::from_utf8_lossy(&page.data));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        page,
    )
        .into_response()
}

async fn computers_page() -> Response {
    file("app.html")
}

async fn asset(request: Request) -> Response {
    file(request.uri().path().trim_start_matches('/'))
}

fn file(path: &str) -> Response {
    let Some(found) = Dist::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = mime_guess::from_path(path).first_or_octet_stream();
    // Hashed bundle files never change; the pages must, so a new build is picked up.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };

    (
        [(header::CONTENT_TYPE, kind.as_ref()), (header::CACHE_CONTROL, cache)],
        found.data,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
