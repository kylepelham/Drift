//! Images and PDFs a tool returns for the model to look at. A tool hands them over as
//! `images: [{mime, data}]` in its metadata (PDFs too, as `application/pdf`); the turn moves the bytes
//! to the blob table and keeps `{mime, hash}`, and the request loads them again, only the newest few,
//! and only for a model that reads that kind.

use crate::session::types::{ToolImage, ToolMetadata};
use base64::Engine as _;

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

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("its data is not valid base64")]
    InvalidBase64,
    #[error("it is over {} MB", MAX_SOURCE_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("it could not be read ({0})")]
    ReadIo(std::io::Error),
    #[error("it could not be read ({0})")]
    Read(image::ImageError),
    #[error("it could not be decoded ({0})")]
    Decode(image::ImageError),
    #[error("its data is corrupt ({0}), so providers refuse it; re-encode the file to look at it")]
    Corrupt(String),
    #[error("at {width}x{height} it could not be scaled under {} MB", MAX_IMAGE_BYTES / 1024 / 1024)]
    CannotScale { width: u32, height: u32 },
}

/// One image as a tool returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub mime: String,
    pub base64: String,
}

impl Image {
    pub fn from_bytes(mime: &str, bytes: &[u8]) -> Self {
        Self {
            mime: mime.into(),
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub fn bytes(&self) -> Option<Vec<u8>> {
        base64::engine::general_purpose::STANDARD.decode(&self.base64).ok()
    }
}

/// `image` as a model can take it, with a note when it had to be scaled; a PDF passes through.
pub fn normalize(image: Image) -> Result<(Image, Option<String>), ImageError> {
    if image.mime == PDF {
        return Ok((image, None));
    }

    let bytes = image.bytes().ok_or(ImageError::InvalidBase64)?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(ImageError::TooLarge);
    }
    check(&bytes)?;
    let (width, height) = reader(&bytes)?.into_dimensions().map_err(ImageError::Read)?;
    if width <= MAX_SIDE && height <= MAX_SIDE && image.base64.len() <= MAX_IMAGE_BYTES {
        return Ok((image, None));
    }

    let decoded = reader(&bytes)?.decode().map_err(ImageError::Decode)?;
    sizes(width, height)
        .find_map(|(to_width, to_height)| {
            encoded(&decoded.resize_exact(to_width, to_height, image::imageops::FilterType::Lanczos3)).map(|image| {
                let note = format!("scaled from {width}x{height} to {to_width}x{to_height}");
                (image, Some(note))
            })
        })
        .ok_or(ImageError::CannotScale { width, height })
}

/// Decodes `bytes` whole: a lenient decoder draws corrupt data that providers refuse, so JPEG is strict.
pub fn check(bytes: &[u8]) -> Result<(), ImageError> {
    if sniff(bytes) == Some("image/jpeg") {
        return check_jpeg(bytes);
    }

    reader(bytes)?.decode().map(drop).map_err(ImageError::Decode)
}

/// `image` decodes JPEG through zune-jpeg in lenient mode; strict mode refuses a broken entropy stream.
fn check_jpeg(bytes: &[u8]) -> Result<(), ImageError> {
    let side = MAX_DECODED_SIDE as usize;
    let options = zune_core::options::DecoderOptions::default()
        .set_strict_mode(true)
        .set_max_width(side)
        .set_max_height(side);

    let decoded =
        zune_jpeg::JpegDecoder::new_with_options(zune_core::bytestream::ZCursor::new(bytes), options).decode();

    decoded
        .map(drop)
        .map_err(|error| ImageError::Corrupt(format!("{error:?}").trim_matches('"').to_string()))
}

fn reader(bytes: &[u8]) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, ImageError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(ImageError::ReadIo)?;

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    reader.limits(limits);
    Ok(reader)
}

/// Fits within `MAX_SIDE`, then shrinks by a quarter each step until it is one pixel.
fn sizes(width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    let scale = (MAX_SIDE as f64 / width as f64)
        .min(MAX_SIDE as f64 / height as f64)
        .min(1.0);
    let first = (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    );

    std::iter::successors(Some(first), |&(width, height)| {
        (width > 1 || height > 1).then(|| ((width * 3 / 4).max(1), (height * 3 / 4).max(1)))
    })
    .take(32)
}

/// Opaque pictures as JPEG first (photos stay small), transparent ones as PNG first so the transparency survives.
fn encoded(picture: &image::DynamicImage) -> Option<Image> {
    let transparent = picture.color().has_alpha() && picture.to_rgba8().pixels().any(|pixel| pixel[3] < u8::MAX);
    if transparent {
        png(picture).or_else(|| jpeg(picture))
    } else {
        jpeg(picture).or_else(|| png(picture))
    }
}

fn png(picture: &image::DynamicImage) -> Option<Image> {
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(Image::from_bytes("image/png", &png)).filter(|image| image.base64.len() <= MAX_IMAGE_BYTES)
}

/// Transparent areas become white, not black, since JPEG has no alpha.
fn jpeg(picture: &image::DynamicImage) -> Option<Image> {
    let rgba = picture.to_rgba8();
    let rgb = image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [red, green, blue, alpha] = rgba.get_pixel(x, y).0;
        let over_white = |channel: u8| ((channel as u16 * alpha as u16 + 255 * (255 - alpha as u16)) / 255) as u8;
        image::Rgb([over_white(red), over_white(green), over_white(blue)])
    });

    JPEG_QUALITIES.iter().find_map(|&quality| {
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
        image::ImageEncoder::write_image(
            encoder,
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
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
pub fn metadata(images: &[Image]) -> Vec<ToolImage> {
    images
        .iter()
        .map(|image| ToolImage {
            mime: image.mime.clone(),
            data: Some(image.base64.clone()),
            hash: None,
            extra: Default::default(),
        })
        .collect()
}

/// The images a tool's metadata hands over, before they are stored.
pub fn returned(metadata: &ToolMetadata) -> Vec<Image> {
    metadata
        .images
        .iter()
        .flatten()
        .filter_map(|image| {
            Some(Image {
                mime: image.mime.clone(),
                base64: image.data.clone()?,
            })
        })
        .collect()
}

/// `stored` as a call's saved metadata.
pub fn stored_metadata(stored: &[Stored]) -> Vec<ToolImage> {
    stored
        .iter()
        .map(|image| ToolImage {
            mime: image.mime.clone(),
            data: None,
            hash: Some(image.hash.clone()),
            extra: Default::default(),
        })
        .collect()
}

/// The images a stored call names.
pub fn stored(metadata: Option<&ToolMetadata>) -> Vec<Stored> {
    metadata
        .into_iter()
        .flat_map(|metadata| metadata.images.iter().flatten())
        .filter_map(|image| {
            Some(Stored {
                mime: image.mime.clone(),
                hash: image.hash.clone()?,
            })
        })
        .collect()
}

/// A valid JPEG with its entropy data broken as a mangled extraction breaks it: a stray `FF 09` in the scan.
#[cfg(test)]
pub(crate) fn corrupt_jpeg() -> Vec<u8> {
    let picture = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, (x ^ y) as u8]));
    let mut bytes = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .unwrap();

    // Past the start-of-scan header, into the compressed data.
    let scan = bytes.windows(2).position(|pair| pair == [0xFF, 0xDA]).unwrap();
    let header = usize::from(u16::from_be_bytes([bytes[scan + 2], bytes[scan + 3]]));
    let inside = scan + 2 + header + 40;
    bytes[inside..inside + 2].copy_from_slice(&[0xFF, 0x09]);

    bytes
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
        assert_eq!(
            returned(&ToolMetadata {
                images: Some(metadata(std::slice::from_ref(&image))),
                ..Default::default()
            }),
            vec![image.clone()]
        );
        assert_eq!(image.bytes().as_deref(), Some(&b"\x89PNG"[..]));
        let saved = Stored {
            mime: "image/png".into(),
            hash: "abc".into(),
        };
        assert_eq!(
            stored(Some(&ToolMetadata {
                images: Some(stored_metadata(std::slice::from_ref(&saved))),
                ..Default::default()
            })),
            vec![saved]
        );
        assert!(stored(None).is_empty());
    }

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let picture = image::RgbImage::from_fn(width, height, |x, y| image::Rgb(pixel(x, y)));
        let mut bytes = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
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
    fn a_corrupt_jpeg_that_decodes_leniently_is_refused() {
        let corrupt = corrupt_jpeg();
        assert!(
            image::load_from_memory(&corrupt).is_ok(),
            "the lenient decoder draws it anyway, which is how it used to get through"
        );

        let refused = normalize(Image::from_bytes("image/jpeg", &corrupt))
            .unwrap_err()
            .to_string();

        assert!(refused.starts_with("its data is corrupt ("), "{refused}");
        assert!(refused.ends_with("re-encode the file to look at it"), "{refused}");
    }

    #[test]
    fn whole_images_pass_the_full_decode_and_a_cut_off_one_does_not() {
        let picture = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(32, 16, image::Rgb([20, 90, 160])));
        for format in [image::ImageFormat::Jpeg, image::ImageFormat::Png] {
            let mut bytes = Vec::new();
            picture.write_to(&mut std::io::Cursor::new(&mut bytes), format).unwrap();

            assert!(check(&bytes).is_ok(), "{format:?}");
        }

        let mut cut = png(40, 30, |x, y| [x as u8, y as u8, 7]).bytes().unwrap();
        cut.truncate(cut.len() / 2);

        assert!(check(&cut).is_err());
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
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed.to_le_bytes()
        };
        let pixels: Vec<[u8; 4]> = (0..1600 * 1600).map(|_| noise()).collect();
        let large = png(1600, 1600, |x, y| {
            let pixel = pixels[(y * 1600 + x) as usize];
            [pixel[0], pixel[1], pixel[2]]
        });
        assert!(large.base64.len() > MAX_IMAGE_BYTES);
        let (image, note) = normalize(large).unwrap();
        assert!(image.base64.len() <= MAX_IMAGE_BYTES);
        assert!(note.unwrap().starts_with("scaled from 1600x1600"));
    }

    #[test]
    fn opaque_pictures_become_jpeg_and_transparent_ones_stay_png() {
        let (photo, _) = normalize(png(2400, 1200, |x, y| [(x % 256) as u8, (y % 256) as u8, 90])).unwrap();
        assert_eq!(photo.mime, "image/jpeg");
        let picture = image::RgbaImage::from_fn(2400, 100, |x, _| {
            image::Rgba([200, 10, 10, if x < 1200 { 0 } else { 255 }])
        });
        let mut bytes = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let (cutout, _) = normalize(Image::from_bytes("image/png", &bytes)).unwrap();
        assert_eq!(cutout.mime, "image/png", "transparency survives");
        let flattened = jpeg(&image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0, 0, 0, 0]),
        )))
        .unwrap();
        let decoded = image::load_from_memory(&flattened.bytes().unwrap()).unwrap().to_rgb8();
        assert!(
            decoded.get_pixel(4, 4).0.iter().all(|channel| *channel > 240),
            "transparent areas turn white, not black"
        );
    }

    #[test]
    fn images_that_cannot_be_decoded_are_refused_with_a_reason() {
        let broken = Image {
            mime: "image/png".into(),
            base64: Image::from_bytes("image/png", b"\x89PNG\r\n\x1a\nbroken").base64,
        };
        assert!(
            normalize(broken)
                .unwrap_err()
                .to_string()
                .starts_with("it could not be")
        );
        assert_eq!(
            normalize(Image {
                mime: "image/png".into(),
                base64: "%%%".into()
            })
            .unwrap_err()
            .to_string(),
            "its data is not valid base64"
        );
    }
}
