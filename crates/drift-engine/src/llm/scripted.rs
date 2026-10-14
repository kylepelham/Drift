use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;

use super::{Block, Chunk, ChunkStream, Credential, Error, Request};

#[derive(Debug)]
enum Response {
    Chunks(Vec<Chunk>),
    Fail(Error),
    /// Streams the chunks, then the error, as an in-stream error frame does.
    FailMidway(Vec<Chunk>, Error),
    /// Streams the chunks after a pause, as a slow reply does.
    Slow(std::time::Duration, Vec<Chunk>),
    /// Streams the first chunks, pauses, then the rest, as a reply still being written does.
    Paused(Vec<Chunk>, std::time::Duration, Vec<Chunk>),
    /// A response that never finishes, for exercising Stop.
    Stall,
}

type Responses = Arc<Mutex<VecDeque<Response>>>;

#[derive(Clone, Debug, Default)]
pub struct Scripted {
    responses: Responses,
    /// Answers for a particular conversation, found by a phrase in its first message; used before
    /// the shared queue, so sessions running at once each get their own replies in their own order.
    keyed: Arc<Mutex<Vec<(String, Response)>>>,
    pub requests: Arc<Mutex<Vec<Request>>>,
    /// The credential each request was sent with, in order.
    pub credentials: Arc<Mutex<Vec<Credential>>>,
}

impl Scripted {
    pub fn push(&self, chunks: Vec<Chunk>) -> &Self {
        self.responses.lock().unwrap().push_back(Response::Chunks(chunks));

        self
    }

    pub fn push_error(&self, error: Error) -> &Self {
        self.responses.lock().unwrap().push_back(Response::Fail(error));

        self
    }

    pub fn push_fail_midway(&self, chunks: Vec<Chunk>, error: Error) -> &Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(Response::FailMidway(chunks, error));

        self
    }

    pub fn push_slow(&self, delay: std::time::Duration, chunks: Vec<Chunk>) -> &Self {
        self.responses.lock().unwrap().push_back(Response::Slow(delay, chunks));

        self
    }

    pub fn push_paused(&self, before: Vec<Chunk>, pause: std::time::Duration, after: Vec<Chunk>) -> &Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(Response::Paused(before, pause, after));

        self
    }

    pub fn push_stall(&self) -> &Self {
        self.responses.lock().unwrap().push_back(Response::Stall);

        self
    }

    /// A reply for the conversation whose first message contains `phrase`.
    pub fn push_for(&self, phrase: &str, chunks: Vec<Chunk>) -> &Self {
        self.keyed
            .lock()
            .unwrap()
            .push((phrase.into(), Response::Chunks(chunks)));

        self
    }

    pub fn push_slow_for(&self, phrase: &str, delay: std::time::Duration, chunks: Vec<Chunk>) -> &Self {
        self.keyed
            .lock()
            .unwrap()
            .push((phrase.into(), Response::Slow(delay, chunks)));

        self
    }

    pub fn push_stall_for(&self, phrase: &str) -> &Self {
        self.keyed.lock().unwrap().push((phrase.into(), Response::Stall));

        self
    }

    pub fn responses_left(&self) -> usize {
        self.responses.lock().unwrap().len() + self.keyed.lock().unwrap().len()
    }

    pub fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        self.requests.lock().unwrap().push(request.clone());
        self.credentials.lock().unwrap().push(credential.clone());

        let first = first_message_text(request);
        let mut keyed = self.keyed.lock().unwrap();
        let found = keyed
            .iter()
            .position(|(phrase, _)| first.contains(phrase.as_str()))
            .map(|index| keyed.remove(index).1);
        drop(keyed);

        match found.or_else(|| self.responses.lock().unwrap().pop_front()) {
            Some(response) => play(response),
            None => Err(Error::Transport("scripted provider has no more responses".into())),
        }
    }
}

fn first_message_text(request: &Request) -> String {
    let Some(message) = request.messages.first() else {
        return String::new();
    };

    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn play(response: Response) -> Result<ChunkStream, Error> {
    match response {
        Response::Chunks(chunks) => Ok(Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok)))),
        Response::Fail(error) => Err(error),
        Response::FailMidway(chunks, error) => Ok(Box::pin(futures_util::stream::iter(
            chunks.into_iter().map(Ok).chain([Err(error)]),
        ))),
        Response::Slow(delay, chunks) => {
            let later = futures_util::stream::once(tokio::time::sleep(delay))
                .flat_map(move |()| futures_util::stream::iter(chunks.clone().into_iter().map(Ok)));

            Ok(Box::pin(later))
        }
        Response::Paused(before, pause, after) => {
            let rest = futures_util::stream::once(tokio::time::sleep(pause))
                .flat_map(move |()| futures_util::stream::iter(after.clone().into_iter().map(Ok)));

            Ok(Box::pin(
                futures_util::stream::iter(before.into_iter().map(Ok)).chain(rest),
            ))
        }
        Response::Stall => Ok(Box::pin(futures_util::stream::pending())),
    }
}
