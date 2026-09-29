//! Server-sent events over a byte stream: yields (event, data) pairs, tolerant of split chunks.

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
    event: String,
    data: Vec<String>,
}

impl Parser {
    /// Feeds bytes and returns every event completed by them.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
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

pub fn events<S>(bytes: S) -> impl Stream<Item = Result<SseEvent, reqwest::Error>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>>,
{
    let mut parser = Parser::default();
    bytes.flat_map(move |chunk| {
        let items: Vec<Result<SseEvent, reqwest::Error>> = match chunk {
            Ok(bytes) => parser.feed(&bytes).into_iter().map(Ok).collect(),
            Err(error) => vec![Err(error)],
        };
        futures_util::stream::iter(items)
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
    fn joins_multiline_data_and_ignores_comments() {
        let mut parser = Parser::default();
        let events = parser.feed(b": keepalive\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(events, vec![SseEvent { event: String::new(), data: "a\nb".into() }]);
    }
}
