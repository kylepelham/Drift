//! Server-sent events over a byte stream: yields (event, data) pairs, tolerant of split chunks.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};

#[derive(Debug, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct Parser {
    buffer: String,
    /// The start of a character whose remaining bytes are in the next network read.
    pending: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl Parser {
    /// Feeds bytes and returns every event completed by them.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.decode(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.find('\n') {
            let line = self.buffer[..end].trim_end_matches('\r').to_string();
            self.buffer.drain(..=end);
            if let Some(event) = self.line(&line) {
                events.push(event);
            }
        }
        events
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
            self.buffer.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
            let Some(bad) = error.error_len() else {
                self.pending.drain(..valid);
                return;
            };
            self.buffer.push('\u{FFFD}');
            self.pending.drain(..valid + bad);
        }
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

/// Events from a byte stream. Nothing at all for `idle` (not even a comment or ping) means the
/// connection has stalled: the stream ends with an error rather than waiting forever.
pub fn events<S>(bytes: S, idle: Duration) -> impl Stream<Item = Result<SseEvent, String>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut parser = Parser::default();
    watched(bytes, idle).flat_map(move |chunk| {
        let items: Vec<Result<SseEvent, String>> = match chunk {
            Ok(bytes) => parser.feed(&bytes).into_iter().map(Ok).collect(),
            Err(error) => vec![Err(error)],
        };
        futures_util::stream::iter(items)
    })
}

/// A response body that ends with an error once nothing arrives for `idle`.
pub fn watched<S>(bytes: S, idle: Duration) -> impl Stream<Item = Result<Bytes, String>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    futures_util::stream::unfold(Some(Box::pin(bytes)), move |state| async move {
        let mut bytes = state?;
        match tokio::time::timeout(idle, bytes.next()).await {
            Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(bytes))),
            Ok(Some(Err(error))) => Some((Err(error.to_string()), None)),
            Ok(None) => None,
            Err(_) => Some((Err(format!("the stream stalled: nothing for {} s", idle.as_secs())), None)),
        }
    })
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
                SseEvent { event: "message_start".into(), data: "{\"a\":1}".into() },
                SseEvent { event: "ping".into(), data: "{}".into() },
            ]
        );
    }

    #[test]
    fn characters_split_across_reads_arrive_whole_at_every_split() {
        let frames = "event: content_block_delta\ndata: {\"delta\":{\"type\":\"text_delta\",\"text\":\"LEFT € RIGHT 日本 🎉\"}}\n\n\
                      event: content_block_delta\ndata: {\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\": \\\"ü/café.rs\\\"}\"}}\n\n";
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
            bytes.iter().flat_map(|b| parser.feed(std::slice::from_ref(b))).collect()
        };
        assert_eq!(one_by_one, whole, "one byte per read");
        assert!(whole[0].data.contains("LEFT € RIGHT 日本 🎉") && whole[1].data.contains("café"));
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
        let error = tokio::time::timeout(Duration::from_secs(2), events.next()).await.expect("the idle limit ends the wait").unwrap().unwrap_err();
        assert!(error.contains("stalled"), "{error}");
        assert!(events.next().await.is_none(), "and nothing follows");
    }

    #[test]
    fn joins_multiline_data_and_ignores_comments() {
        let mut parser = Parser::default();
        let events = parser.feed(b": keepalive\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(events, vec![SseEvent { event: String::new(), data: "a\nb".into() }]);
    }
}
