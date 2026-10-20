//! A conversation's browser: what it shows, ending it, and the view socket the pane watches and drives it through.

use std::sync::Arc;

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::Lease;
use super::error::ApiError;
use crate::Engine;
use crate::browser::{BrowserRecord, Instance, ViewInput};

/// The conversation's browser, or null when it has none running.
#[utoipa::path(get, path = "/sessions/{id}/browser", operation_id = "getBrowser", responses((status = 200, body = Option<BrowserRecord>)))]
pub(super) async fn get(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Json<Option<BrowserRecord>> {
    Json(engine.browsers.record(&id))
}

/// Ends the conversation's browser; `browser.updated` says when it has gone.
#[utoipa::path(post, path = "/sessions/{id}/browser/stop", operation_id = "stopBrowser", responses((status = 200, body = BrowserRecord), (status = 404)))]
pub(super) async fn stop(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<BrowserRecord>, ApiError> {
    let instance = engine.browsers.get(&id).ok_or_else(|| ApiError::not_found("browser"))?;
    instance.stop();

    Ok(Json(instance.record()))
}

/// Frames of the active page while the socket is open, as binary messages (a 4-byte little-endian header length,
/// a JSON header, a JPEG); the client sends `ViewInput` as JSON text.
#[utoipa::path(
    get,
    path = "/sessions/{id}/browser/view",
    operation_id = "browserView",
    responses((status = 101, description = "WebSocket; binary frames out, ViewInput in"), (status = 404))
)]
pub(super) async fn view(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    lease: Option<axum::Extension<Lease>>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let instance = engine.browsers.get(&id).ok_or_else(|| ApiError::not_found("browser"))?;
    let lease = lease.map(|axum::Extension(Lease(token))| token).unwrap_or_default();

    Ok(ws.on_upgrade(move |socket| watch(instance, socket, lease)))
}

async fn watch(instance: Arc<Instance>, mut socket: WebSocket, lease: CancellationToken) {
    let mut viewer = instance.watch();

    // Input is applied in the order it was sent: a click's press must reach the page before its release.
    let (inputs, mut queued) = mpsc::unbounded_channel::<ViewInput>();
    let driving = instance.clone();
    let applier = tokio::spawn(async move {
        while let Some(input) = queued.recv().await {
            let _ = driving.input(input).await;
        }
    });

    loop {
        tokio::select! {
            () = lease.cancelled() => break,
            () = instance.stopped() => break,
            frame = viewer.frames.recv() => match frame {
                Ok(frame) => {
                    if socket.send(Message::Binary(frame.encode().into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(input) = serde_json::from_str::<ViewInput>(&text) {
                        let _ = inputs.send(input);
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }

    applier.abort();
}
