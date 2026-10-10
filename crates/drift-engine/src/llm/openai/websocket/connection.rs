//! The task that owns one connection: it runs the conversation's requests in order and remembers the last
//! completed response, so the next request sends only what is new.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::cache;
use super::socket::{self, Socket};
use crate::llm::http::Timeouts;
use crate::llm::openai::stream::StreamState;
use crate::llm::{Chunk, Error};

/// One request and where its chunks go.
pub(super) struct Job {
    pub body: Value,
    pub timeouts: Timeouts,
    pub send: mpsc::Sender<Result<Chunk, Error>>,
}

/// Serves requests until the conversation stops sending them, is idle for `idle`, or the socket fails.
pub(super) async fn run(mut socket: Socket, mut jobs: mpsc::Receiver<Job>, idle: Duration) {
    let mut previous = None;

    while let Some(job) = next_job(&mut socket, &mut jobs, idle).await {
        let Err(error) = response(&mut socket, &job, &mut previous).await else {
            continue;
        };

        // The server forgets a chain that failed; an API error leaves the socket usable, anything else does not.
        previous = None;
        let usable = matches!(error, Error::Api { .. });
        let _ = job.send.send(Err(error)).await;
        if !usable {
            return;
        }
    }
}

/// Waits for the next request, answering heartbeats while the turn runs its tools.
async fn next_job(socket: &mut Socket, jobs: &mut mpsc::Receiver<Job>, idle: Duration) -> Option<Job> {
    let deadline = Instant::now() + idle;

    loop {
        tokio::select! {
            job = jobs.recv() => return job,
            frame = socket.next() => {
                if !socket::idle(socket, frame).await {
                    return None;
                }
            }
            () = tokio::time::sleep_until(deadline) => return None,
        }
    }
}

/// Sends one request and forwards its events as chunks until it completes.
async fn response(socket: &mut Socket, job: &Job, previous: &mut Option<cache::Previous>) -> Result<(), Error> {
    let payload = cache::payload(&job.body, previous.as_ref());
    socket::send(socket, &payload, job.timeouts.headers).await?;

    let mut state = StreamState::default();
    let mut wait = job.timeouts.headers;
    let mut continued = payload.get("previous_response_id").is_some();
    let mut forwarded = false;

    loop {
        let event = socket::receive(socket, &job.send, wait).await?;
        wait = job.timeouts.idle;

        // The server no longer holds the previous response: send the whole history once, on the same socket.
        if continued && !forwarded && error_code(&event) == Some("previous_response_not_found") {
            *previous = None;
            continued = false;
            socket::send(socket, &cache::payload(&job.body, None), job.timeouts.headers).await?;

            state = StreamState::default();
            wait = job.timeouts.headers;
            continue;
        }

        let chunks = decode(&mut state, &event)?;
        forwarded |= !chunks.is_empty();
        for chunk in chunks {
            job.send
                .send(Ok(chunk))
                .await
                .map_err(|_| Error::Transport("the Responses stream was stopped".into()))?;
        }

        if finished(previous, &job.body, &event) {
            return Ok(());
        }
    }
}

fn decode(state: &mut StreamState, event: &Value) -> Result<Vec<Chunk>, Error> {
    if error_code(event) == Some("websocket_connection_limit_reached") {
        return Err(Error::Transport(
            "the Responses WebSocket reached its connection lifetime; reconnecting".into(),
        ));
    }

    state.chunks_value(event)
}

/// True at the end of a response; a completed one becomes the next request's starting point.
fn finished(previous: &mut Option<cache::Previous>, body: &Value, event: &Value) -> bool {
    match event["type"].as_str() {
        Some("response.completed") => *previous = cache::Previous::completed(body, &event["response"]),
        Some("response.incomplete") => *previous = None,
        _ => return false,
    }

    true
}

fn error_code(event: &Value) -> Option<&str> {
    if event["type"] != "error" {
        return None;
    }

    event["error"]["code"].as_str().or(event["code"].as_str())
}
