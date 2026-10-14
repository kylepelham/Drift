//! Frame-level reads and writes on an open Responses WebSocket, each bounded by a deadline.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use super::IDLE;
use crate::llm::{Chunk, Error};

pub(super) type Socket = WebSocketStream<reqwest::Upgraded>;
type Frame = Option<Result<Message, tokio_tungstenite::tungstenite::Error>>;

/// The largest event accepted; a completed response repeats its whole output.
const MAX_EVENT: usize = 16 * 1024 * 1024;

pub(super) async fn upgraded(stream: reqwest::Upgraded) -> Socket {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_EVENT))
        .max_frame_size(Some(MAX_EVENT));

    WebSocketStream::from_raw_socket(stream, Role::Client, Some(config)).await
}

pub(super) async fn send(socket: &mut Socket, payload: &Value, timeout: Duration) -> Result<(), Error> {
    let frame = Message::Text(payload.to_string().into());

    tokio::time::timeout(timeout, socket.send(frame))
        .await
        .map_err(|_| Error::Transport("sending the Responses WebSocket request timed out".into()))?
        .map_err(|error| Error::Transport(format!("Responses WebSocket: {error}")))
}

/// Answers a heartbeat between requests; false when the connection should close.
pub(super) async fn idle(socket: &mut Socket, frame: Frame) -> bool {
    match frame {
        Some(Ok(Message::Ping(_))) => {
            let flushed = tokio::time::timeout(IDLE, socket.flush()).await;
            flushed.is_ok_and(|result| result.is_ok())
        }
        Some(Ok(Message::Pong(_))) => true,
        _ => false,
    }
}

/// The next event, answering heartbeats on the way; fails if the reader stopped listening or nothing came in time.
pub(super) async fn receive(
    socket: &mut Socket,
    reader: &mpsc::Sender<Result<Chunk, Error>>,
    wait: Duration,
) -> Result<Value, Error> {
    let deadline = Instant::now() + wait;

    loop {
        let frame = tokio::select! {
            frame = tokio::time::timeout_at(deadline, socket.next()) => {
                frame.map_err(|_| silence(wait))?
            }
            () = reader.closed() => return Err(Error::Transport("the Responses stream was stopped".into())),
        };

        match frame {
            Some(Ok(Message::Text(text))) => return parse(serde_json::from_str(&text)),
            Some(Ok(Message::Binary(bytes))) => return parse(serde_json::from_slice(&bytes)),
            Some(Ok(Message::Ping(_))) => heartbeat(socket, deadline).await?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Err(error)) => return Err(Error::Transport(format!("Responses WebSocket: {error}"))),
            _ => {
                return Err(Error::Transport(
                    "the Responses WebSocket closed before response.completed".into(),
                ));
            }
        }
    }
}

async fn heartbeat(socket: &mut Socket, deadline: Instant) -> Result<(), Error> {
    tokio::time::timeout_at(deadline, socket.flush())
        .await
        .map_err(|_| Error::Transport("sending the Responses WebSocket heartbeat timed out".into()))?
        .map_err(|error| Error::Transport(error.to_string()))
}

fn parse(event: serde_json::Result<Value>) -> Result<Value, Error> {
    event.map_err(|error| Error::Malformed(error.to_string()))
}

fn silence(wait: Duration) -> Error {
    Error::Transport(format!(
        "Responses WebSocket received no event within {} s",
        wait.as_secs()
    ))
}
