//! AWS event-stream framing (`application/vnd.amazon.eventstream`):
//! length-prefixed messages with string headers and CRC32 checks.

use std::collections::HashMap;

/// One decoded message: its string headers and payload.
#[derive(Debug, PartialEq)]
pub(super) struct Message {
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

const PRELUDE: usize = 12;
const TRAILER: usize = 4;
/// Header value type 7 is a string; the only kind these services send that matters here.
const STRING: u8 = 7;

#[derive(Default)]
pub(super) struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    /// Feeds bytes and returns every message they complete; a bad frame is an error, as nothing after it is trusted.
    pub(super) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Message>, FrameError> {
        self.buffer.extend_from_slice(bytes);
        let mut messages = Vec::new();

        while self.buffer.len() >= PRELUDE {
            let total = u32::from_be_bytes(self.buffer[0..4].try_into().unwrap()) as usize;
            if total < PRELUDE + TRAILER {
                return Err(FrameError::TooShort { bytes: total });
            }
            if self.buffer.len() < total {
                break;
            }

            let frame: Vec<u8> = self.buffer.drain(..total).collect();
            messages.push(decode(&frame)?);
        }

        Ok(messages)
    }
}

fn decode(frame: &[u8]) -> Result<Message, FrameError> {
    let word = |at: usize| u32::from_be_bytes(frame[at..at + 4].try_into().unwrap());
    if crc32fast::hash(&frame[..8]) != word(8) {
        return Err(FrameError::PreludeChecksum);
    }

    let end = frame.len() - TRAILER;
    if crc32fast::hash(&frame[..end]) != word(end) {
        return Err(FrameError::MessageChecksum);
    }

    let headers_end = PRELUDE + word(4) as usize;
    if headers_end > end {
        return Err(FrameError::HeadersOverrun);
    }

    Ok(Message {
        headers: headers(&frame[PRELUDE..headers_end])?,
        payload: frame[headers_end..end].to_vec(),
    })
}

fn headers(mut bytes: &[u8]) -> Result<HashMap<String, String>, FrameError> {
    let mut out = HashMap::new();

    while !bytes.is_empty() {
        let name_len = bytes[0] as usize;
        let name_bytes = bytes.get(1..1 + name_len).ok_or(FrameError::TruncatedHeader("name"))?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        bytes = &bytes[1 + name_len..];

        let kind = *bytes.first().ok_or(FrameError::TruncatedHeader("type"))?;
        let size = value_size(kind, bytes.get(1..3))?;
        let value = bytes.get(1..1 + size).ok_or(FrameError::TruncatedHeader("value"))?;
        if kind == STRING {
            out.insert(name, String::from_utf8_lossy(&value[2..]).into_owned());
        }
        bytes = &bytes[1 + size..];
    }

    Ok(out)
}

/// Bytes a header value takes after its type byte, length prefixes included.
fn value_size(kind: u8, length: Option<&[u8]>) -> Result<usize, FrameError> {
    Ok(match kind {
        0 | 1 => 0,
        2 => 1,
        3 => 2,
        4 => 4,
        5 | 8 => 8,
        9 => 16,
        6 | 7 => {
            let length = length.ok_or(FrameError::TruncatedHeader("length"))?;
            2 + u16::from_be_bytes(length.try_into().unwrap()) as usize
        }
        other => return Err(FrameError::UnknownHeaderType(other)),
    })
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub(super) enum FrameError {
    #[error("event-stream frame of {bytes} bytes is too short")]
    TooShort { bytes: usize },
    #[error("event-stream prelude checksum mismatch")]
    PreludeChecksum,
    #[error("event-stream message checksum mismatch")]
    MessageChecksum,
    #[error("event-stream headers overrun the frame")]
    HeadersOverrun,
    #[error("truncated header {0}")]
    TruncatedHeader(&'static str),
    #[error("unknown event-stream header type {0}")]
    UnknownHeaderType(u8),
}

/// Builds a frame as the service would; for tests and recorded exchanges.
#[cfg(test)]
pub(super) fn frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut head = Vec::new();
    for (name, value) in headers {
        head.push(name.len() as u8);
        head.extend_from_slice(name.as_bytes());
        head.push(STRING);
        head.extend_from_slice(&(value.len() as u16).to_be_bytes());
        head.extend_from_slice(value.as_bytes());
    }

    let total = (PRELUDE + head.len() + payload.len() + TRAILER) as u32;
    let mut out = total.to_be_bytes().to_vec();
    out.extend_from_slice(&(head.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc32fast::hash(&out).to_be_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32fast::hash(&out).to_be_bytes());

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_anywhere_decode_whole() {
        let one = frame(
            &[(":message-type", "event"), (":event-type", "chunk")],
            br#"{"bytes":"e30="}"#,
        );
        let two = frame(
            &[
                (":message-type", "exception"),
                (":exception-type", "throttlingException"),
            ],
            br#"{"message":"slow down"}"#,
        );
        let stream: Vec<u8> = one.iter().chain(two.iter()).copied().collect();
        for split in 0..stream.len() {
            let mut decoder = Decoder::default();
            let mut messages = decoder.feed(&stream[..split]).unwrap();
            messages.extend(decoder.feed(&stream[split..]).unwrap());
            assert_eq!(messages.len(), 2, "split at {split}");
            assert_eq!(messages[0].headers[":event-type"], "chunk");
            assert_eq!(messages[1].headers[":exception-type"], "throttlingException");
            assert_eq!(messages[1].payload, br#"{"message":"slow down"}"#);
        }
    }

    #[test]
    fn a_corrupted_frame_is_refused() {
        let mut bad = frame(&[(":event-type", "chunk")], b"{}");
        let last = bad.len() - 6;
        bad[last] ^= 1;
        let error = Decoder::default().feed(&bad).unwrap_err();
        assert_eq!(error, FrameError::MessageChecksum);
        assert_eq!(error.to_string(), "event-stream message checksum mismatch");
    }
}
