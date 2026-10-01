//! Images a tool returns for the model to look at: kept on the call's metadata as `images`,
//! replayed after the call's result, and replaced by a line for a model that cannot read them.

use base64::Engine as _;
use serde_json::{json, Value};

/// Larger images are refused rather than sent: providers reject them (Anthropic's limit is 5 MB).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// One image as a call's metadata holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub mime: String,
    pub base64: String,
}

impl Image {
    pub fn from_bytes(mime: &str, bytes: &[u8]) -> Self {
        Self { mime: mime.into(), base64: base64::engine::general_purpose::STANDARD.encode(bytes) }
    }
}

/// The image type `bytes` start with, for the formats every image-reading provider takes.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// `images` as call metadata.
pub fn metadata(images: &[Image]) -> Value {
    json!(images.iter().map(|image| json!({ "mime": image.mime, "data": image.base64 })).collect::<Vec<_>>())
}

/// The images a call's metadata holds.
pub fn from_metadata(metadata: Option<&Value>) -> Vec<Image> {
    let Some(Value::Array(images)) = metadata.map(|m| &m["images"]) else { return Vec::new() };
    images
        .iter()
        .filter_map(|image| Some(Image { mime: image["mime"].as_str()?.into(), base64: image["data"].as_str()?.into() }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_known_by_their_bytes_and_round_trip_through_metadata() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"plain text"), None);
        let image = Image::from_bytes("image/png", b"\x89PNG");
        let stored = json!({ "images": metadata(std::slice::from_ref(&image)) });
        assert_eq!(from_metadata(Some(&stored)), vec![image]);
        assert!(from_metadata(None).is_empty());
    }
}
