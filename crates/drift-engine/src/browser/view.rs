//! What the pane sees and sends: frames of the active page while it watches, and the user's input.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::input::{KeyInput, MouseInput};

/// One picture of the active page.
pub struct Frame {
    pub page: String,
    /// The page's size in CSS pixels, which pane coordinates map onto.
    pub width: f64,
    pub height: f64,
    pub jpeg: Vec<u8>,
}

#[derive(Serialize)]
struct Header<'a> {
    page: &'a str,
    width: f64,
    height: f64,
}

impl Frame {
    /// One binary message: a 4-byte little-endian header length, the JSON header, then the JPEG.
    pub fn encode(&self) -> Vec<u8> {
        let header = Header {
            page: &self.page,
            width: self.width,
            height: self.height,
        };
        let header = serde_json::to_vec(&header).expect("a frame header always serialises");
        let length = u32::try_from(header.len()).expect("a frame header is short");

        let mut message = Vec::with_capacity(4 + header.len() + self.jpeg.len());
        message.extend_from_slice(&length.to_le_bytes());
        message.extend_from_slice(&header);
        message.extend_from_slice(&self.jpeg);

        message
    }
}

/// What the pane sends on its view socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ViewInput {
    #[serde(rename = "mouse")]
    Mouse(MouseInput),
    #[serde(rename = "key")]
    Key(KeyInput),
    /// Pasted or composed text, typed into the focused element.
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "select")]
    Select { page: String },
    #[serde(rename = "close")]
    Close { page: String },
    #[serde(rename = "newPage")]
    NewPage {},
    /// An address to load in the active page, or `back`, `forward`, `reload`.
    #[serde(rename = "navigate")]
    Navigate { url: String },
    #[serde(rename = "dialog")]
    Dialog { accept: bool, text: Option<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_header_length_header_and_picture() {
        let frame = Frame {
            page: "T1".into(),
            width: 1280.0,
            height: 800.0,
            jpeg: vec![0xFF, 0xD8],
        };
        let message = frame.encode();

        let length = u32::from_le_bytes(message[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&message[4..4 + length]).unwrap();
        assert_eq!(header["page"], "T1");
        assert_eq!(header["width"], 1280.0);
        assert_eq!(&message[4 + length..], &[0xFF, 0xD8]);
    }

    #[test]
    fn the_panes_messages_parse() {
        let mouse: ViewInput =
            serde_json::from_str(r#"{"type":"mouse","event":"down","x":1,"y":2,"clickCount":1}"#).unwrap();
        assert!(matches!(mouse, ViewInput::Mouse(MouseInput { ref event, .. }) if event == "down"));

        let dialog: ViewInput = serde_json::from_str(r#"{"type":"dialog","accept":true}"#).unwrap();
        assert_eq!(
            dialog,
            ViewInput::Dialog {
                accept: true,
                text: None
            }
        );
    }
}
