//! Images a tool returns for the model to look at. A tool hands them over as `images: [{mime, data}]`
//! in its metadata; the turn moves the bytes to the blob table and keeps `{mime, hash}`, and the
//! request loads them again, only the newest few, and only for a model that reads images.

use base64::Engine as _;
use serde_json::{json, Value};

/// Larger images are refused rather than sent: providers reject them (Anthropic's limit is 5 MB).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The formats every image-reading provider takes; anything else (SVG, BMP, ...) is named, not sent.
pub const SENDABLE: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// One image as a tool returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub mime: String,
    pub base64: String,
}

impl Image {
    pub fn from_bytes(mime: &str, bytes: &[u8]) -> Self {
        Self { mime: mime.into(), base64: base64::engine::general_purpose::STANDARD.encode(bytes) }
    }

    pub fn bytes(&self) -> Option<Vec<u8>> {
        base64::engine::general_purpose::STANDARD.decode(&self.base64).ok()
    }
}

/// An image as a stored call names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub mime: String,
    pub hash: String,
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

/// `images` as a tool's metadata.
pub fn metadata(images: &[Image]) -> Value {
    json!(images.iter().map(|image| json!({ "mime": image.mime, "data": image.base64 })).collect::<Vec<_>>())
}

/// The images a tool's metadata hands over, before they are stored.
pub fn returned(metadata: &Value) -> Vec<Image> {
    let Value::Array(images) = &metadata["images"] else { return Vec::new() };
    images.iter().filter_map(|image| Some(Image { mime: image["mime"].as_str()?.into(), base64: image["data"].as_str()?.into() })).collect()
}

/// `stored` as a call's saved metadata.
pub fn stored_metadata(stored: &[Stored]) -> Value {
    json!(stored.iter().map(|image| json!({ "mime": image.mime, "hash": image.hash })).collect::<Vec<_>>())
}

/// The images a stored call names.
pub fn stored(metadata: Option<&Value>) -> Vec<Stored> {
    let Some(Value::Array(images)) = metadata.map(|m| &m["images"]) else { return Vec::new() };
    images.iter().filter_map(|image| Some(Stored { mime: image["mime"].as_str()?.into(), hash: image["hash"].as_str()?.into() })).collect()
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
        assert_eq!(returned(&json!({ "images": metadata(std::slice::from_ref(&image)) })), vec![image.clone()]);
        assert_eq!(image.bytes().as_deref(), Some(&b"\x89PNG"[..]));
        let saved = Stored { mime: "image/png".into(), hash: "abc".into() };
        assert_eq!(stored(Some(&json!({ "images": stored_metadata(std::slice::from_ref(&saved)) }))), vec![saved]);
        assert!(stored(None).is_empty());
    }
}
