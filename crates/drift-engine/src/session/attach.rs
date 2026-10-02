//! A prompt's files, made usable before the prompt is admitted. An @ mention of a file is read in
//! only where the read tool would read it without asking; otherwise the model is told to use that
//! tool, which asks. Text travels as text, an image only to a model that reads images, and anything
//! else is refused with the reason: nothing is silently dropped.

use std::path::{Path, PathBuf};

use base64::Engine as _;

use super::turn::TurnError;
use super::types::Part;
use crate::llm::catalog::Model;
use crate::permission::{Decision, Policy};

/// A mentioned file's text is cut here, with a note on where to read on.
const MAX_MENTION_BYTES: usize = crate::tool::spool::MAX_RESULT_BYTES;
const MAX_LISTED: usize = 1000;

/// What deciding about a prompt's files needs.
pub struct Attach<'a> {
    pub engine: &'a crate::Engine,
    pub session_id: &'a str,
    pub workspace: &'a Path,
    pub policy: &'a Policy,
    /// The model the prompt will actually run on (for a steered prompt, the running turn's).
    pub model: &'a Model,
}

/// A prompt's parts ready to admit, and the mentions read in full: once admitted, those count as
/// read, so the model can edit them.
pub struct Prepared {
    pub parts: Vec<Part>,
    pub read: Vec<PathBuf>,
}

impl Attach<'_> {
    pub fn prepare(&self, parts: Vec<Part>) -> Result<Prepared, TurnError> {
        let mut read = Vec::new();
        let parts = parts.into_iter().map(|part| self.part(part, &mut read)).collect::<Result<_, _>>()?;
        Ok(Prepared { parts, read })
    }

    fn part(&self, part: Part, read: &mut Vec<PathBuf>) -> Result<Part, TurnError> {
        let Part::File { mime, name, url, .. } = part else { return Ok(part) };
        if let Some(path) = file_path(&url) {
            let shown = display_name(self.workspace, &path);
            return Ok(mention_part(&shown, &self.mention(&path, read)));
        }
        let refuse = |why: String| Err(TurnError::Attachment(format!("{name}: {why}")));
        let Some(data) = DataUrl::parse(&url) else { return refuse("only files and data URLs can be attached".into()) };
        if !data.mime.is_empty() && !data.mime.eq_ignore_ascii_case(&mime) {
            return refuse(format!("it says it is {mime} but its data is {}", data.mime));
        }
        match mime.split('/').next().unwrap_or_default() {
            "text" if data.text().is_some() => Ok(Part::File { mime, name, url, path: None }),
            "text" => refuse("its text could not be decoded".into()),
            "image" if !data.base64 || data.bytes().is_none() => refuse("its image data is not valid base64".into()),
            "image" if self.model.attachment => Ok(Part::File { mime, name, url, path: None }),
            "image" => Err(TurnError::Attachment(format!("{} cannot read images; pick a model that can, or remove {name}", self.model.name))),
            _ if mime.eq_ignore_ascii_case(crate::tool::image::PDF) => self.pdf(mime, name, &url, &data),
            _ => refuse(format!("{mime} cannot be sent to a model yet; attach it as text, or as an image the model can read")),
        }
    }

    /// A PDF goes whole to a model that reads PDFs; one that does not, or bytes that are no PDF, are refused with the reason.
    fn pdf(&self, mime: String, name: String, url: &str, data: &DataUrl) -> Result<Part, TurnError> {
        let refuse = |why: String| Err(TurnError::Attachment(format!("{name}: {why}")));
        match data.bytes().filter(|_| data.base64) {
            None => refuse("its PDF data is not valid base64".into()),
            Some(bytes) if !bytes.starts_with(b"%PDF-") => refuse("it says it is a PDF but its data is not one".into()),
            Some(_) if !self.model.pdf => Err(TurnError::Attachment(format!("{} cannot read PDFs; pick a model that can, or remove {name}", self.model.name))),
            Some(_) => Ok(Part::File { mime, name, url: url.to_string(), path: None }),
        }
    }

    /// The mentioned file's text, or a note saying why it was not read.
    fn mention(&self, path: &Path, read_whole: &mut Vec<PathBuf>) -> String {
        let shown = display_name(self.workspace, path);
        if let Some(ask) = crate::tool::read_ask(self.workspace, path, "Read") {
            match self.engine.permissions.decide_now(self.session_id, self.policy, &ask) {
                Decision::Allow => {}
                Decision::Deny => return format!("[@{shown} was mentioned but a rule forbids reading it.]"),
                Decision::Ask => return format!("[@{shown} was mentioned but not read: {}. Use the read tool, which asks the user first.]", why(self.workspace, path)),
            }
        }
        match read(path) {
            Ok(Read::Whole(text)) => {
                read_whole.push(path.to_path_buf());
                format!("<file path=\"{shown}\">\n{text}\n</file>")
            }
            // Cut short, it does not count as read: the model has not seen the whole file.
            Ok(Read::Partial(text) | Read::Listing(text)) => format!("<file path=\"{shown}\">\n{text}\n</file>"),
            Err(reason) => format!("[@{shown} was mentioned but {reason}.]"),
        }
    }
}

enum Read {
    Whole(String),
    Partial(String),
    Listing(String),
}

/// A `data:` URL taken apart; `mime` is empty when the URL does not say.
struct DataUrl<'a> {
    mime: &'a str,
    base64: bool,
    payload: &'a str,
}

impl<'a> DataUrl<'a> {
    fn parse(url: &'a str) -> Option<Self> {
        let (header, payload) = url.strip_prefix("data:")?.split_once(',')?;
        let mut params = header.split(';');
        let mime = params.next().unwrap_or_default().trim();
        let base64 = params.any(|p| p.trim().eq_ignore_ascii_case("base64"));
        Some(Self { mime, base64, payload })
    }

    fn bytes(&self) -> Option<Vec<u8>> {
        if self.base64 {
            return base64::engine::general_purpose::STANDARD.decode(self.payload.trim()).ok();
        }
        Some(percent_encoding::percent_decode_str(self.payload).collect())
    }

    fn text(&self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }
}

fn why(workspace: &Path, path: &Path) -> &'static str {
    if crate::tool::sensitive::is_sensitive(path) {
        "it may hold secrets"
    } else if !path.starts_with(workspace) {
        "it is outside the workspace"
    } else {
        "reading it needs approval"
    }
}

/// A file's text within the mention bound, or a directory's entries; `Err` says why neither. Only
/// one byte past the bound is ever read, however large the file.
fn read(path: &Path) -> Result<Read, String> {
    use std::io::Read as _;
    let meta = std::fs::metadata(path).map_err(|_| "it does not exist".to_string())?;
    if meta.is_dir() {
        return Ok(Read::Listing(list(path)));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("it could not be read ({e})"))?;
    let mut bytes = Vec::new();
    file.take(MAX_MENTION_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("it could not be read ({e})"))?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("it is binary".into());
    }
    if bytes.len() <= MAX_MENTION_BYTES {
        return Ok(Read::Whole(String::from_utf8_lossy(&bytes).into_owned()));
    }
    let text = String::from_utf8_lossy(&bytes[..MAX_MENTION_BYTES]);
    let cut = text.rfind('\n').unwrap_or(0);
    let shown_lines = text[..cut].lines().count();
    Ok(Read::Partial(format!("{}\n\n(cut after {shown_lines} lines; read with offset {} for the rest)", &text[..cut], shown_lines + 1)))
}

fn list(path: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| format!("{}{}", entry.file_name().to_string_lossy(), if entry.path().is_dir() { "/" } else { "" }))
        .collect();
    names.sort();
    let total = names.len();
    names.truncate(MAX_LISTED);
    let more = if total > MAX_LISTED { format!("\n({} more entries)", total - MAX_LISTED) } else { String::new() };
    format!("{}{more}", names.join("\n"))
}

/// A mention as the model reads it, a text file, that remembers which workspace file it was.
fn mention_part(shown: &str, text: &str) -> Part {
    let url = format!("data:text/plain;base64,{}", base64::engine::general_purpose::STANDARD.encode(text));
    Part::File { mime: "text/plain".into(), name: shown.into(), url, path: Some(shown.into()) }
}

fn display_name(workspace: &Path, path: &Path) -> String {
    crate::tool::display(path, workspace)
}

/// The resolved path of a `file:` URL, as the UI sends for an @ mention.
fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let decoded = percent_encoding::percent_decode_str(rest).decode_utf8().ok()?;
    let windows_drive = decoded.len() > 2 && decoded.starts_with('/') && decoded.as_bytes()[2] == b':';
    let raw = if windows_drive { &decoded[1..] } else { &decoded[..] };
    Some(crate::tool::canonical(Path::new(raw)))
}

/// A text data URL's content, for the request builder. Admission already refused any that fail here.
pub fn data_text(url: &str) -> Option<String> {
    DataUrl::parse(url)?.text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_resolve_on_both_platforms_and_data_urls_decode() {
        let windows = file_path("file:///C:/repo/my%20notes.md").unwrap();
        assert!(windows.to_string_lossy().replace('\\', "/").ends_with("repo/my notes.md"), "{windows:?}");
        assert!(file_path("https://example.com/a").is_none());
        assert_eq!(data_text("data:text/plain;base64,aGVsbG8="), Some("hello".into()));
        assert_eq!(data_text("data:text/plain;charset=utf-8;base64,aGVsbG8="), Some("hello".into()));
        assert_eq!(data_text("data:text/plain,a%20b"), Some("a b".into()));
        assert_eq!(data_text("data:text/plain;base64,not base64!"), None);
        assert_eq!(data_text("data:text/plain;base64,/w=="), None, "not UTF-8");
    }

    #[test]
    fn a_large_file_is_read_only_up_to_the_bound() {
        let dir = std::env::temp_dir().join(format!("drift-mention-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.txt");
        std::fs::write(&big, "line\n".repeat(MAX_MENTION_BYTES)).unwrap();
        let Ok(Read::Partial(text)) = read(&big) else { panic!("a file past the bound is partial") };
        assert!(text.len() < MAX_MENTION_BYTES + 200 && text.ends_with("for the rest)"));
        std::fs::write(&big, "small\n").unwrap();
        assert!(matches!(read(&big), Ok(Read::Whole(text)) if text == "small\n"));
        std::fs::remove_dir_all(dir).ok();
    }
}
