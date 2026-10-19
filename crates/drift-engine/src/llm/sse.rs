//! Server-sent events over a byte stream: yields (event, data) pairs, tolerant of split chunks. The same
//! reader yields plain lines for newline-delimited JSON.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};

#[derive(Debug, PartialEq)]
pub(crate) struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub(crate) struct Parser {
    lines: LineReader,
    event: String,
    data: Vec<String>,
}

/// Complete lines of a byte stream, whatever reads split a line or a character.
#[derive(Default)]
pub(crate) struct LineReader {
    buffer: String,
    /// The start of a character whose remaining bytes are in the next network read.
    pending: Vec<u8>,
}

impl Parser {
    /// Feeds bytes and returns every event completed by them.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let lines = self.lines.feed(chunk);

        lines.into_iter().filter_map(|line| self.line(&line)).collect()
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.flush();
        }

        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = value.to_string(),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }

        None
    }

    fn flush(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_empty() {
            return None;
        }

        let event = SseEvent {
            event: std::mem::take(&mut self.event),
            data: std::mem::take(&mut self.data).join("\n"),
        };
        Some(event)
    }
}

impl LineReader {
    /// Feeds bytes and returns every line completed by them, without its line ending.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.decode(chunk);
        let mut lines = Vec::new();

        while let Some(end) = self.buffer.find('\n') {
            lines.push(self.buffer[..end].trim_end_matches('\r').to_string());
            self.buffer.drain(..=end);
        }

        lines
    }

    /// Appends complete characters to the buffer and keeps an unfinished one for the next read; bytes
    /// that can never form a character become U+FFFD.
    fn decode(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);

        loop {
            let error = match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    self.buffer.push_str(text);
                    self.pending.clear();
                    return;
                }
                Err(error) => error,
            };

            let valid = error.valid_up_to();
            self.buffer
                .push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
            let Some(bad) = error.error_len() else {
                self.pending.drain(..valid);
                return;
            };

            self.buffer.push('\u{FFFD}');
            self.pending.drain(..valid + bad);
        }
    }
}

/// Events from a byte stream. Nothing at all for `idle` (not even a comment or ping) means the
/// connection has stalled: the stream ends with an error rather than waiting forever.
pub(crate) fn events<S>(bytes: S, idle: Duration) -> impl Stream<Item = Result<SseEvent, StreamError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut parser = Parser::default();

    watched(bytes, idle).flat_map(move |chunk| {
        let items: Vec<Result<SseEvent, StreamError>> = match chunk {
            Ok(bytes) => parser.feed(&bytes).into_iter().map(Ok).collect(),
            Err(error) => vec![Err(error)],
        };

        futures_util::stream::iter(items)
    })
}

/// The non-empty lines of a byte stream (newline-delimited JSON), with the same stall limit as [`events`].
pub(crate) fn lines<S>(bytes: S, idle: Duration) -> impl Stream<Item = Result<String, StreamError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut reader = LineReader::default();

    watched(bytes, idle).flat_map(move |chunk| {
        let items: Vec<Result<String, StreamError>> = match chunk {
            Ok(bytes) => reader
                .feed(&bytes)
                .into_iter()
                .filter(|line| !line.trim().is_empty())
                .map(Ok)
                .collect(),
            Err(error) => vec![Err(error)],
        };

        futures_util::stream::iter(items)
    })
}

/// A response body that ends with an error once nothing arrives for `idle`.
pub(crate) fn watched<S>(bytes: S, idle: Duration) -> impl Stream<Item = Result<Bytes, StreamError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    futures_util::stream::unfold(Some(Box::pin(bytes)), move |state| async move {
        let mut bytes = state?;
        match tokio::time::timeout(idle, bytes.next()).await {
            Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(bytes))),
            Ok(Some(Err(error))) => Some((Err(StreamError::Transport(error)), None)),
            Ok(None) => None,
            Err(_) => Some((
                Err(StreamError::Stalled {
                    seconds: idle.as_secs(),
                }),
                None,
            )),
        }
    })
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StreamError {
    /// reqwest calls every failed body read a decode error; the innermost cause says what happened.
    #[error("connection lost while reading the response: {}", innermost(.0))]
    Transport(reqwest::Error),
    #[error("the stream stalled: nothing for {seconds} s")]
    Stalled { seconds: u64 },
}

fn innermost(error: &(dyn std::error::Error + 'static)) -> String {
    std::iter::successors(Some(error), |error| error.source())
        .last()
        .map_or_else(String::new, ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut parser = Parser::default();
        assert!(parser.feed(b"event: message_start\ndata: {\"a\":").is_empty());
        let events = parser.feed(b"1}\n\nevent: ping\ndata: {}\n\n");
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: "message_start".into(),
                    data: "{\"a\":1}".into()
                },
                SseEvent {
                    event: "ping".into(),
                    data: "{}".into()
                },
            ]
        );
    }

    #[test]
    fn characters_split_across_reads_arrive_whole_at_every_split() {
        let frames = "event: content_block_delta\n\
                      data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"LEFT € RIGHT 日本 \u{1f389}\"}}\n\n\
                      event: content_block_delta\n\
                      data: {\"delta\":{\"type\":\"input_json_delta\",\
                      \"partial_json\":\"{\\\"path\\\": \\\"ü/café.rs\\\"}\"}}\n\n";
        let bytes = frames.as_bytes();
        let whole = Parser::default().feed(bytes);
        for split in 1..bytes.len() {
            let mut parser = Parser::default();
            let mut events = parser.feed(&bytes[..split]);
            events.extend(parser.feed(&bytes[split..]));
            assert_eq!(events, whole, "split at byte {split}");
        }
        let one_by_one: Vec<SseEvent> = {
            let mut parser = Parser::default();
            bytes
                .iter()
                .flat_map(|byte| parser.feed(std::slice::from_ref(byte)))
                .collect()
        };
        assert_eq!(one_by_one, whole, "one byte per read");
        assert!(whole[0].data.contains("LEFT € RIGHT 日本 \u{1f389}") && whole[1].data.contains("café"));
    }

    #[test]
    fn bytes_that_can_never_be_a_character_are_replaced_not_held() {
        let mut parser = Parser::default();
        let events = parser.feed(b"data: a\xffb\n\n");
        assert_eq!(events[0].data, "a\u{FFFD}b");
    }

    #[tokio::test]
    async fn a_stream_that_goes_quiet_ends_with_an_error() {
        let first: Result<Bytes, reqwest::Error> = Ok(Bytes::from_static(b"data: one\n\n"));
        let stalled = futures_util::stream::iter([first]).chain(futures_util::stream::pending());
        let mut events = Box::pin(events(stalled, Duration::from_millis(100)));
        assert_eq!(events.next().await.unwrap().unwrap().data, "one");
        let error = tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .expect("the idle limit ends the wait")
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, StreamError::Stalled { seconds: 0 }), "{error}");
        assert_eq!(error.to_string(), "the stream stalled: nothing for 0 s");
        assert!(events.next().await.is_none(), "and nothing follows");
    }

    #[tokio::test]
    async fn a_connection_lost_midway_is_reported_as_one_not_as_a_decode_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0; 4096]).await;
            let head = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nb\r\ndata: one\n\n\r\n";
            socket.write_all(head).await.unwrap();
        });

        let response = crate::llm::http::client().get(&url).send().await.unwrap();
        let mut events = Box::pin(events(response.bytes_stream(), Duration::from_secs(5)));
        assert_eq!(events.next().await.unwrap().unwrap().data, "one");
        let error = events.next().await.unwrap().unwrap_err().to_string();
        assert!(
            error.starts_with("connection lost while reading the response: ") && !error.contains("decoding"),
            "{error}"
        );
    }

    #[test]
    fn joins_multiline_data_and_ignores_comments() {
        let mut parser = Parser::default();
        let events = parser.feed(b": keepalive\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(
            events,
            vec![SseEvent {
                event: String::new(),
                data: "a\nb".into()
            }]
        );
    }
}
