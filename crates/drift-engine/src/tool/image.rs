//! Images and PDFs a tool returns for the model to look at. A tool hands them over as
//! `images: [{mime, data}]` in its metadata (PDFs too, as `application/pdf`); the turn moves the bytes
//! to the blob table and keeps `{mime, hash}`, and the request loads them again, only the newest few,
//! and only for a model that reads that kind.

use base64::Engine as _;
use serde_json::{json, Value};

/// An image's base64 size a provider accepts (Anthropic's limit is 5 MB); larger ones are scaled down.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The longest side sent; providers downscale or reject beyond it.
pub const MAX_SIDE: u32 = 2000;
/// Larger source images are refused before decoding.
pub const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_DECODED_SIDE: u32 = 16_384;
const JPEG_QUALITIES: [u8; 4] = [85, 70, 55, 40];
/// Larger PDFs are refused: providers cap the whole request, and a PDF counts against it whole.
pub const MAX_PDF_BYTES: usize = 10 * 1024 * 1024;
pub const PDF: &str = "application/pdf";

/// Whether `bytes` are a PDF.
pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

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

/// `image` as a model can take it, with a note when it had to be scaled; a PDF passes through.
pub fn normalize(image: Image) -> Result<(Image, Option<String>), String> {
    if image.mime == PDF {
        return Ok((image, None));
    }
    let bytes = image.bytes().ok_or("its data is not valid base64")?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(format!("it is over {} MB", MAX_SOURCE_BYTES / 1024 / 1024));
    }
    let (width, height) = reader(&bytes)?.into_dimensions().map_err(|e| format!("it could not be read ({e})"))?;
    if width <= MAX_SIDE && height <= MAX_SIDE && image.base64.len() <= MAX_IMAGE_BYTES {
        return Ok((image, None));
    }
    let decoded = reader(&bytes)?.decode().map_err(|e| format!("it could not be decoded ({e})"))?;
    sizes(width, height)
        .find_map(|(w, h)| encoded(&decoded.resize_exact(w, h, image::imageops::FilterType::Lanczos3)).map(|image| (image, Some(format!("scaled from {width}x{height} to {w}x{h}")))))
        .ok_or_else(|| format!("at {width}x{height} it could not be scaled under {} MB", MAX_IMAGE_BYTES / 1024 / 1024))
}

fn reader(bytes: &[u8]) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| format!("it could not be read ({e})"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    reader.limits(limits);
    Ok(reader)
}

/// Fits within `MAX_SIDE`, then shrinks by a quarter each step until it is one pixel.
fn sizes(width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    let scale = (MAX_SIDE as f64 / width as f64).min(MAX_SIDE as f64 / height as f64).min(1.0);
    let first = (((width as f64 * scale).round() as u32).max(1), ((height as f64 * scale).round() as u32).max(1));
    std::iter::successors(Some(first), |&(w, h)| (w > 1 || h > 1).then(|| ((w * 3 / 4).max(1), (h * 3 / 4).max(1)))).take(32)
}

fn encoded(picture: &image::DynamicImage) -> Option<Image> {
    let mut png = Vec::new();
    if picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_ok() {
        let image = Image::from_bytes("image/png", &png);
        if image.base64.len() <= MAX_IMAGE_BYTES {
            return Some(image);
        }
    }
    let rgb = picture.to_rgb8();
    JPEG_QUALITIES.iter().find_map(|&quality| {
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
        image::ImageEncoder::write_image(encoder, rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8).ok()?;
        Some(Image::from_bytes("image/jpeg", &jpeg)).filter(|image| image.base64.len() <= MAX_IMAGE_BYTES)
    })
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

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let picture = image::RgbImage::from_fn(width, height, |x, y| image::Rgb(pixel(x, y)));
        let mut bytes = Vec::new();
        picture.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        Image::from_bytes("image/png", &bytes)
    }

    fn dimensions(image: &Image) -> (u32, u32) {
        reader(&image.bytes().unwrap()).unwrap().into_dimensions().unwrap()
    }

    #[test]
    fn images_within_the_limits_are_sent_unchanged() {
        let small = png(40, 30, |_, _| [9, 9, 9]);
        assert_eq!(normalize(small.clone()).unwrap(), (small, None));
        let pdf = Image::from_bytes(PDF, b"%PDF-1.7");
        assert_eq!(normalize(pdf.clone()).unwrap(), (pdf, None));
    }

    #[test]
    fn oversized_dimensions_are_scaled_to_fit_and_keep_their_shape() {
        let (image, note) = normalize(png(2400, 600, |x, _| [(x % 256) as u8, 0, 0])).unwrap();
        assert_eq!(dimensions(&image), (2000, 500));
        assert_eq!(note.as_deref(), Some("scaled from 2400x600 to 2000x500"));
    }

    #[test]
    fn oversized_bytes_are_reencoded_under_the_provider_limit() {
        let mut seed = 0x2545_f491_u32;
        let mut noise = || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed.to_le_bytes() };
        let pixels: Vec<[u8; 4]> = (0..1600 * 1600).map(|_| noise()).collect();
        let large = png(1600, 1600, |x, y| { let p = pixels[(y * 1600 + x) as usize]; [p[0], p[1], p[2]] });
        assert!(large.base64.len() > MAX_IMAGE_BYTES);
        let (image, note) = normalize(large).unwrap();
        assert!(image.base64.len() <= MAX_IMAGE_BYTES);
        assert!(note.unwrap().starts_with("scaled from 1600x1600"));
    }

    #[test]
    fn images_that_cannot_be_decoded_are_refused_with_a_reason() {
        let broken = Image { mime: "image/png".into(), base64: Image::from_bytes("image/png", b"\x89PNG\r\n\x1a\nbroken").base64 };
        assert!(normalize(broken).unwrap_err().starts_with("it could not be"));
        assert_eq!(normalize(Image { mime: "image/png".into(), base64: "%%%".into() }).unwrap_err(), "its data is not valid base64");
    }
}
