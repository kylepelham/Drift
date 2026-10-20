//! What the pane sees and sends: frames of the active page while it watches, and the user's input.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::input::{KeyInput, MouseInput};

/// The narrowest and widest layouts a pane gives a page; a narrower pane shows it scaled down, so pages keep their
/// desktop layout.
const PANE_WIDTHS: (f64, f64) = (1280.0, 3840.0);
/// The tallest and widest shapes a pane may give a page, as height over width.
const PANE_SHAPES: (f64, f64) = (0.4, 2.5);

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
    /// The size in CSS pixels the pane shows the page at, so the page can be laid out to fill it.
    #[serde(rename = "fit")]
    Fit { width: f64, height: f64 },
}

/// The viewport that fills a pane of this size: a desktop-wide layout in the pane's shape.
pub(crate) fn pane_viewport(width: f64, height: f64) -> Option<(u64, u64)> {
    if !(width.is_finite() && height.is_finite() && width >= 1.0 && height >= 1.0) {
        return None;
    }

    let layout = width.clamp(PANE_WIDTHS.0, PANE_WIDTHS.1);
    let shape = (height / width).clamp(PANE_SHAPES.0, PANE_SHAPES.1);

    Some((layout.round() as u64, (layout * shape).round() as u64))
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

    #[test]
    fn a_pane_gets_a_desktop_wide_page_in_its_own_shape() {
        assert_eq!(pane_viewport(640.0, 800.0), Some((1280, 1600)));
        assert_eq!(pane_viewport(1600.0, 900.0), Some((1600, 900)));
        assert_eq!(pane_viewport(400.0, 4000.0), Some((1280, 3200)));
        assert_eq!(pane_viewport(5000.0, 2500.0), Some((3840, 1920)));
        assert_eq!(pane_viewport(0.0, 800.0), None);
        assert_eq!(pane_viewport(f64::NAN, 800.0), None);
        assert_eq!(pane_viewport(f64::INFINITY, 800.0), None);
    }
}
