//! Text files are UTF-8 or UTF-16: decoded for the model, and written back in the encoding they had.
//! UTF-16's byte order mark decodes as U+FEFF, as UTF-8's does, so the code that keeps a BOM keeps either.

use std::path::Path;

/// How far into a file without a byte order mark its bytes are looked at to tell UTF-16.
const SNIFF_BYTES: usize = 8000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Encoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    pub(crate) fn encode(self, text: &str) -> Vec<u8> {
        let units = text.encode_utf16();
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Utf16Le => units.flat_map(u16::to_le_bytes).collect(),
            Self::Utf16Be => units.flat_map(u16::to_be_bytes).collect(),
        }
    }

    /// Its name for the model, for any encoding but UTF-8.
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            Self::Utf8 => None,
            Self::Utf16Le => Some("UTF-16 LE"),
            Self::Utf16Be => Some("UTF-16 BE"),
        }
    }
}

/// `bytes` as text and the encoding they are in, or `None` for bytes that are neither UTF-8 nor UTF-16.
pub(crate) fn decode(bytes: &[u8]) -> Option<(String, Encoding)> {
    if let Some(encoding) = utf16(bytes) {
        return decode_utf16(bytes, encoding).map(|text| (text, encoding));
    }

    String::from_utf8(bytes.to_vec())
        .ok()
        .map(|text| (text, Encoding::Utf8))
}

/// A file's text when it is UTF-8 or UTF-16, without a byte order mark, for files read as configuration or
/// instructions.
pub(crate) fn read_to_string(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    match decode(&bytes) {
        Some((text, _)) => Ok(text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text)),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not UTF-8 or UTF-16 text",
        )),
    }
}

/// The UTF-16 byte order of `bytes`: by their byte order mark, else when most of their first units are ASCII,
/// whose other byte is zero. Binary data almost always holds a zero unit, which UTF-16 text never does.
fn utf16(bytes: &[u8]) -> Option<Encoding> {
    match bytes {
        [0xFF, 0xFE, 0, 0, ..] => return None,
        [0xFF, 0xFE, ..] => return Some(Encoding::Utf16Le),
        [0xFE, 0xFF, ..] => return Some(Encoding::Utf16Be),
        _ => {}
    }
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }

    let sample = &bytes[..bytes.len().min(SNIFF_BYTES) & !1];
    let units = sample.len() / 2;
    let pairs = || sample.chunks_exact(2);
    if pairs().any(|pair| pair == [0, 0]) {
        return None;
    }
    let ascii = |zero: usize| pairs().filter(|pair| pair[zero] == 0).count() * 2 >= units;

    match (ascii(1), ascii(0)) {
        (true, false) => Some(Encoding::Utf16Le),
        (false, true) => Some(Encoding::Utf16Be),
        _ => None,
    }
}

fn decode_utf16(bytes: &[u8], encoding: Encoding) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| match encoding {
            Encoding::Utf16Be => u16::from_be_bytes([pair[0], pair[1]]),
            _ => u16::from_le_bytes([pair[0], pair[1]]),
        })
        .collect();

    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::{Encoding, decode};

    #[test]
    fn utf16_is_told_by_its_byte_order_mark_or_its_ascii() {
        for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
            let marked = encoding.encode("\u{feff}héllo\r\nwörld\r\n");
            assert_eq!(decode(&marked), Some(("\u{feff}héllo\r\nwörld\r\n".into(), encoding)));
            let bare = encoding.encode("plain text\n");
            assert_eq!(decode(&bare), Some(("plain text\n".into(), encoding)));
        }
        assert_eq!(decode("plain".as_bytes()), Some(("plain".into(), Encoding::Utf8)));
        assert_eq!(
            decode("\u{feff}x".as_bytes()),
            Some(("\u{feff}x".into(), Encoding::Utf8))
        );
    }

    #[test]
    fn binary_and_other_encodings_are_not_text() {
        let zip = decode(&[0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00]);
        assert_eq!(
            zip.map(|(_, encoding)| encoding),
            Some(Encoding::Utf8),
            "a zip is not UTF-16"
        );
        assert_eq!(decode(&[0x63, 0x61, 0x66, 0xE9]), None, "Windows-1252");
        assert_eq!(decode(&[0xFF, 0xFE, 0x00, 0xD8]), None, "an unpaired surrogate");
        assert_eq!(decode(&[0xFF, 0xFE, 0x41]), None, "an odd byte out");
        assert_eq!(decode(&[0xFF, 0xFE, 0, 0, 0x41, 0, 0, 0]), None, "UTF-32");
    }

    #[test]
    fn an_instruction_file_written_as_utf16_reads_without_its_mark() {
        let path = crate::platform::temp::test_run().join(format!("AGENTS-{}.md", crate::random_hex(4)));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, Encoding::Utf16Le.encode("\u{feff}# Rules\r\n")).unwrap();

        assert_eq!(super::read_to_string(&path).unwrap(), "# Rules\r\n");
    }

    #[test]
    fn text_with_little_ascii_is_utf16_only_by_its_mark() {
        let japanese = Encoding::Utf16Le.encode("日本語のテキスト");
        assert_eq!(decode(&japanese).map(|(_, encoding)| encoding), None);
        let marked = Encoding::Utf16Le.encode("\u{feff}日本語");
        assert_eq!(decode(&marked).unwrap().1, Encoding::Utf16Le);
    }
}
