//! Passes the UI's requests to the chosen PC with this device's session, and its event socket through as is.

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
/// What the WebView sends that is this proxy's, not the PC's: the PC gets the device's session instead.
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
    let logout = request.uri().path() == "/auth/logout";
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
        drop_route(&proxy, &route);
        return refuse(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };

    if logout && response.status().is_success() {
        forget(&proxy, &route.id);
    }
    relay(response)
}

/// Signing out from the UI removes the PC from this device too: its session is gone.
fn forget(proxy: &Proxy, id: &str) {
    proxy.route.lock().unwrap().take_if(|current| current.id == id);
    let _ = proxy.computers.lock().unwrap().remove(id);
}

/// Clears the route a failed request went by, unless another computer or address was chosen meanwhile.
fn drop_route(proxy: &Proxy, used: &Route) {
    proxy
        .route
        .lock()
        .unwrap()
        .take_if(|current| current.id == used.id && current.pc.base == used.pc.base);
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
        drop_route(proxy, &route);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc::{Endpoint, Pc};
    use crate::tls::Trust;

    fn route(id: &str, port: u16) -> Route {
        let endpoint = Endpoint::Lan(([127, 0, 0, 1], port).into());
        Route {
            id: id.into(),
            pc: Pc::new(endpoint, Trust::Authority([0u8; 32])),
            token: "session".into(),
        }
    }

    #[test]
    fn a_failed_request_clears_only_the_route_it_went_by() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let path = crate::test_dir().join("drift-forward-route.json");
        let proxy = super::super::Shared::new(
            crate::computers::Computers::load(path.clone()),
            std::sync::Arc::new(NoHost),
            "secret".into(),
            0,
            path.with_extension("storage.json"),
        );
        let (old, switched) = (route("old", 1), route("new", 2));

        *proxy.route.lock().unwrap() = Some(switched.clone());
        drop_route(&proxy, &old);
        assert_eq!(
            proxy.route.lock().unwrap().as_ref().map(|route| route.id.as_str()),
            Some("new")
        );
        drop_route(&proxy, &route("new", 3));
        assert!(
            proxy.route.lock().unwrap().is_some(),
            "the same computer at a newer address stays"
        );
        drop_route(&proxy, &switched);
        assert!(proxy.route.lock().unwrap().is_none());
    }

    struct NoHost;

    impl crate::Host for NoHost {
        fn asset(&self, _: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
            None
        }

        fn device_name(&self) -> String {
            "Phone".into()
        }
    }
}
