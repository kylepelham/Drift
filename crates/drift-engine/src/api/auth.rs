use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;

use crate::Engine;

#[derive(Deserialize)]
pub(super) struct TokenQuery {
    token: Option<String>,
}

/// Bearer header normally; `?token=` for browsers opening a WebSocket, which cannot set headers.
pub(super) async fn require_token(
    State(engine): State<Arc<Engine>>,
    Query(query): Query<TokenQuery>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let presented = header.or(query.token.as_deref());
    if presented.is_some_and(|token| constant_time_eq(token, &engine.token)) {
        return Ok(next.run(request).await);
    }
    Err(StatusCode::UNAUTHORIZED)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut diff = left.len() ^ right.len();
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= (a ^ b) as usize;
    }
    diff == 0
}
