use crate::{remote_auth, ui_state};
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Extension, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use futures_util::StreamExt;
use rust_embed::RustEmbed;
use std::path::{Component, Path};
use std::sync::OnceLock;
use tauri::Manager;
use tokio::sync::{broadcast, watch};
use tower::ServiceExt;

use super::{RemoteAccess, Store, invoke_rpc};

/// Maximum engine request size, including attachments carried in prompts.
const MAX_ENGINE_BODY: usize = drift_engine::api::MAX_REQUEST_BYTES;
const MAX_RPC_BODY: usize = 10 * 1024 * 1024;
static ENGINE_ROUTER: OnceLock<Router> = OnceLock::new();

#[derive(RustEmbed)]
#[folder = "../dist"]
struct FrontendAssets;

pub(super) fn router(app: tauri::AppHandle) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/companion") }))
        .route("/companion", get(static_asset))
        .route("/auth/options", get(remote_auth::options))
        .route("/auth/certificate", get(remote_auth::certificate))
        .route("/auth/link", post(remote_auth::start_link))
        .route("/auth/link/{id}", get(remote_auth::poll_link))
        .route("/auth/login", post(remote_auth::login))
        .route("/auth/logout", post(remote_auth::logout))
        .route("/auth/me", get(remote_auth::me))
        // A wildcard route would leak its path parameter into the engine's own extractors.
        .nest_service(
            "/engine",
            any(native_engine)
                .layer(DefaultBodyLimit::max(MAX_ENGINE_BODY))
                .with_state(app.clone()),
        )
        .route("/api/invoke", post(invoke_rpc))
        .route("/api/ui-state/events", get(ui_state_events))
        .fallback(static_asset)
        .layer(DefaultBodyLimit::max(MAX_RPC_BODY))
        .layer(middleware::from_fn_with_state(app.clone(), gateway_middleware))
        .layer(middleware::from_fn(host_guard))
        .with_state(app)
}

async fn ui_state_events(
    State(app): State<tauri::AppHandle>,
    Extension(auth): Extension<watch::Receiver<u64>>,
) -> Response {
    let authority = app.state::<ui_state::UiStateAuthority>();
    let Ok(initial) = authority.snapshot() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop UI state has not been initialized",
        )
            .into_response();
    };

    let receiver = authority.subscribe();
    let stream = futures_util::stream::unfold(
        (Some(initial), receiver, auth),
        |(initial, mut receiver, mut auth)| async move {
            if let Some(snapshot) = initial {
                let event = Event::default().json_data(snapshot).ok()?;
                return Some((Ok::<_, std::convert::Infallible>(event), (None, receiver, auth)));
            }

            loop {
                tokio::select! {
                    changed = auth.changed() => {
                        let _ = changed;
                        return None;
                    }
                    received = receiver.recv() => match received {
                        Ok(snapshot) => {
                            let event = Event::default().json_data(snapshot).ok()?;
                            return Some((Ok(event), (None, receiver, auth)));
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {},
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            }
        },
    );

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

async fn gateway_middleware(State(app): State<tauri::AppHandle>, mut request: Request, next: Next) -> Response {
    let access = app.state::<RemoteAccess>();
    let path = request.uri().path().to_string();
    let enabled = access.should_start();
    let response = if enabled && remote_auth::public_path(&path) {
        next.run(request).await
    } else if let Some((auth, device)) = access.authorize(request.headers(), &app.state::<Store>()) {
        request.extensions_mut().insert(auth);
        request.extensions_mut().insert(device);
        next.run(request).await
    } else if enabled && request.method() == axum::http::Method::GET && remote_auth::sign_in_path(&path) {
        remote_auth::sign_in_page()
    } else {
        (StatusCode::UNAUTHORIZED, "remote access authentication required").into_response()
    };

    secure(response)
}

/// Checks host and origin before authentication so DNS-rebound pages cannot reach even sign-in routes.
pub(super) async fn host_guard(request: Request, next: Next) -> Response {
    let authority = request.uri().authority().map(|authority| authority.as_str().to_owned());
    if valid_host_origin(request.headers(), authority.as_deref()) {
        return next.run(request).await;
    }

    secure((StatusCode::FORBIDDEN, "invalid host or origin").into_response())
}

/// Validates matching HTTPS host and origin, including HTTP/2 authority from the URI rather than Host.
pub(super) fn valid_host_origin(headers: &HeaderMap, authority: Option<&str>) -> bool {
    let header_host = headers.get(header::HOST).and_then(|value| value.to_str().ok());
    let Some(host) = authority.or(header_host) else {
        return false;
    };
    if host.is_empty() || host.contains(['/', '\\', '@']) {
        return false;
    }

    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };

    origin.scheme() == "https" && origin[url::Position::BeforeHost..url::Position::AfterPort] == *host
}

fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("frame-ancestors 'self'"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=()"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}

async fn static_asset(uri: Uri) -> Response {
    let Some(path) = static_path(uri.path()) else {
        return (StatusCode::BAD_REQUEST, "invalid asset path").into_response();
    };
    let path = if path.is_empty() || path == "companion" {
        "index.html"
    } else {
        path
    };
    let content = asset_content(path).or_else(|| {
        if Path::new(path).extension().is_none() {
            asset_content("index.html")
        } else {
            None
        }
    });
    let Some(content) = content else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Body::from(content).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap());

    response
}

fn asset_content(path: &str) -> Option<Vec<u8>> {
    dev_asset(path).or_else(|| FrontendAssets::get(path).map(|asset| asset.data.into_owned()))
}

fn dev_asset(path: &str) -> Option<Vec<u8>> {
    if !cfg!(debug_assertions) {
        return None;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("dist");
    std::fs::read(root.join(path)).ok()
}

pub(super) fn static_path(path: &str) -> Option<&str> {
    let raw = path.trim_start_matches('/');
    let lower = raw.to_ascii_lowercase();
    if raw.contains('\\') || lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") {
        return None;
    }
    if Path::new(raw)
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        && !raw.is_empty()
    {
        return None;
    }

    Some(raw)
}

/// Serves the native engine under /engine after the gateway strips the mount prefix and authenticates the device.
/// Adds the engine token inside the gateway only; remote devices never receive it.
/// Leases sockets to device credentials so signing out closes them.
async fn native_engine(
    State(app): State<tauri::AppHandle>,
    Extension(mut auth): Extension<watch::Receiver<u64>>,
    mut request: Request,
) -> Response {
    let engine = app.state::<crate::native::Native>().engine().clone();
    let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {}", engine.token)) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    request.headers_mut().insert(header::AUTHORIZATION, bearer);
    request.headers_mut().remove(header::COOKIE);
    let router = ENGINE_ROUTER.get_or_init(|| drift_engine::api::router(engine)).clone();

    // Lease upgraded sockets to the device credentials so sign-out closes them too.
    if request.headers().contains_key(header::UPGRADE) {
        let lease = drift_engine::api::Lease::default();
        request.extensions_mut().insert(lease.clone());
        tokio::spawn(async move {
            let _ = auth.changed().await;
            lease.cancel();
        });
        return router.oneshot(request).await.into_response();
    }

    let response = tokio::select! {
        _ = auth.changed() => return (StatusCode::UNAUTHORIZED, "remote access credentials changed").into_response(),
        response = router.oneshot(request) => response.into_response(),
    };
    let (parts, body) = response.into_parts();
    let stream = revoke_on_auth_change(body.into_data_stream(), auth);

    Response::from_parts(parts, Body::from_stream(stream))
}

pub(super) fn revoke_on_auth_change<S>(
    stream: S,
    mut auth: watch::Receiver<u64>,
) -> impl futures_util::Stream<Item = S::Item>
where
    S: futures_util::Stream,
{
    stream.take_until(async move {
        let _ = auth.changed().await;
    })
}
