//! The UI is served from a dev server or the webview origin, never from the engine's own port.

use axum::http::{HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};

pub fn layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| local_origin(origin)))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

/// Loopback web origins and the Tauri webview; the bearer token still guards every request.
fn local_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else { return false };
    let local = [
        "http://localhost:",
        "http://127.0.0.1:",
        "http://localhost/",
        "http://127.0.0.1/",
    ];
    let webview = ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"];
    local.iter().any(|prefix| origin.starts_with(prefix)) || webview.contains(&origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_and_webview_origins_pass() {
        for allowed in [
            "http://localhost:5180",
            "http://127.0.0.1:4196",
            "tauri://localhost",
            "http://tauri.localhost",
        ] {
            assert!(local_origin(&HeaderValue::from_static(allowed)), "{allowed}");
        }
        for denied in [
            "https://evil.com",
            "http://localhost.evil.com",
            "http://localhostx:5180",
        ] {
            assert!(!local_origin(&HeaderValue::from_static(denied)), "{denied}");
        }
    }
}
