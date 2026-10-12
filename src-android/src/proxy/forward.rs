//! Passes the UI's requests to the chosen PC with this phone's session, and its event socket through as is.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use hyper_util::rt::TokioIo;
use serde_json::json;

use super::{Proxy, Route, Unrouted};

/// Headers that describe one hop, not the request: never passed on, either way.
const HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
/// What the WebView sends that is this proxy's, not the PC's: the PC gets the phone's session instead.
const LOCAL: [&str; 5] = ["host", "cookie", "origin", "authorization", "content-length"];
const UNREACHABLE: &str = "the computer could not be reached";

pub(super) async fn forward(State(proxy): State<Proxy>, request: Request) -> Response {
    let route = match proxy.destination().await {
        Ok(route) => route,
        Err(Unrouted::NoneChosen) => return refuse(StatusCode::SERVICE_UNAVAILABLE, "no computer is selected"),
        Err(Unrouted::Unreachable) => return refuse(StatusCode::BAD_GATEWAY, UNREACHABLE),
    };
    if is_upgrade(request.headers()) {
        return upgrade(&proxy, route, request).await;
    }

    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .to_string();
    let (parts, body) = request.into_parts();
    let mut outgoing = route
        .pc
        .client
        .request(parts.method, route.pc.url(&path))
        .bearer_auth(&route.token)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()));
    for (name, value) in parts.headers.iter().filter(|(name, _)| passed(name, &LOCAL)) {
        outgoing = outgoing.header(name, value);
    }

    let Ok(response) = outgoing.send().await else {
        // The next request looks for the PC again, at any of its addresses.
        proxy.route.lock().unwrap().take();
        return refuse(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };

    if path == "/auth/logout" && response.status().is_success() {
        forget(&proxy, &route.id);
    }
    relay(response)
}

/// Signing out from the UI removes the PC from this phone too: its session is gone.
fn forget(proxy: &Proxy, id: &str) {
    proxy.route.lock().unwrap().take();
    let _ = proxy.computers.lock().unwrap().remove(id);
}

/// The event socket: the PC answers the WebView's own handshake, then bytes flow both ways untouched.
async fn upgrade(proxy: &Proxy, route: Route, mut request: Request) -> Response {
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .to_string();
    let downstream = hyper::upgrade::on(&mut request);
    let mut outgoing = route.pc.client.get(route.pc.url(&path)).bearer_auth(&route.token);
    // The handshake's own headers (Connection, Upgrade, Sec-WebSocket-*) go through; only the proxy's do not.
    for (name, value) in request
        .headers()
        .iter()
        .filter(|(name, _)| !LOCAL[..4].contains(&name.as_str()))
    {
        outgoing = outgoing.header(name, value);
    }

    let Ok(response) = outgoing.send().await else {
        proxy.route.lock().unwrap().take();
        return refuse(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return relay(response);
    }

    let headers = response.headers().clone();
    let Ok(mut upstream) = response.upgrade().await else {
        return refuse(StatusCode::BAD_GATEWAY, "the computer did not open the event stream");
    };
    tokio::spawn(async move {
        if let Ok(downstream) = downstream.await {
            let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(downstream), &mut upstream).await;
        }
    });

    let mut switching = Response::new(Body::empty());
    *switching.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    *switching.headers_mut() = headers;
    switching
}

fn relay(response: reqwest::Response) -> Response {
    let status = response.status();
    let headers = response.headers().clone();
    let body = Body::from_stream(response.bytes_stream().map_err(std::io::Error::other));

    let mut relayed = Response::new(body);
    *relayed.status_mut() = status;
    for (name, value) in headers.iter().filter(|(name, _)| passed(name, &[])) {
        relayed.headers_mut().append(name, value.clone());
    }

    relayed
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

fn passed(name: &HeaderName, local: &[&str]) -> bool {
    let name = name.as_str();
    !HOP.contains(&name) && !local.contains(&name)
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}
