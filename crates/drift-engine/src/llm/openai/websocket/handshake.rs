//! The HTTP/1.1 upgrade that opens a Responses WebSocket, sent through the shared client for its proxy and TLS.

use reqwest::{RequestBuilder, Response, StatusCode};
use tokio_tungstenite::tungstenite::handshake;

use super::Prepared;
use super::socket::{self, Socket};
use crate::llm::Error;

/// The Codex backend accepts the upgrade only with this beta.
const CODEX_BETA: &str = "responses_websockets=2026-02-06";

/// Statuses that mean the endpoint does not speak WebSocket, so SSE is used instead.
const NOT_SUPPORTED: [u16; 4] = [404, 405, 426, 501];

/// The upgraded socket; `None` when the endpoint does not take WebSockets.
pub(super) async fn connect(client: &reqwest::Client, prepared: &Prepared) -> Result<Option<Socket>, Error> {
    let key = handshake::client::generate_key();
    let response = crate::llm::http::send(request(client, prepared, &key), &prepared.timeouts).await?;

    // A refusal for any other reason (credentials, limits) is the request's own error, not a reason to fall back.
    let status = response.status();
    if status != StatusCode::SWITCHING_PROTOCOLS {
        if status.is_success() || NOT_SUPPORTED.contains(&status.as_u16()) {
            return Ok(None);
        }

        let headers = response.headers().clone();
        let text = crate::llm::http::bounded_body(response, &prepared.timeouts).await;
        return Err(crate::llm::openai::api_error(status.as_u16(), &text).with_headers(&headers));
    }

    validate(&response, &key)?;

    let upgraded = tokio::time::timeout(prepared.timeouts.headers, response.upgrade())
        .await
        .map_err(|_| Error::Transport("the Responses WebSocket upgrade timed out".into()))??;

    Ok(Some(socket::upgraded(upgraded).await))
}

fn request(client: &reqwest::Client, prepared: &Prepared, key: &str) -> RequestBuilder {
    let request = client
        .get(prepared.handshake.url().clone())
        .version(reqwest::Version::HTTP_11)
        .headers(prepared.handshake.headers().clone())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", key);

    if !prepared.subscription {
        return request;
    }

    let existing = prepared
        .handshake
        .headers()
        .get("openai-beta")
        .and_then(|value| value.to_str().ok());

    request.header("openai-beta", with_codex_beta(existing))
}

/// The request's own betas with the WebSocket one added once.
fn with_codex_beta(existing: Option<&str>) -> String {
    let Some(existing) = existing else {
        return CODEX_BETA.into();
    };
    if existing.split(',').any(|beta| beta.trim() == CODEX_BETA) {
        return existing.into();
    }

    format!("{existing},{CODEX_BETA}")
}

/// Refuses a 101 that does not prove the server took this upgrade.
fn validate(response: &Response, key: &str) -> Result<(), Error> {
    let header = |name| response.headers().get(name).and_then(|value| value.to_str().ok());
    let accept = handshake::derive_accept_key(key.as_bytes());

    let upgraded = header("upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let connection = header("connection")
        .is_some_and(|value| value.split(',').any(|part| part.trim().eq_ignore_ascii_case("upgrade")));
    let accepted = header("sec-websocket-accept") == Some(accept.as_str());

    if upgraded && connection && accepted {
        return Ok(());
    }

    Err(Error::Malformed(
        "the server returned an invalid WebSocket handshake".into(),
    ))
}
