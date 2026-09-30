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
    pub model: &'a Model,
}

impl Attach<'_> {
    pub fn prepare(&self, parts: Vec<Part>) -> Result<Vec<Part>, TurnError> {
        parts.into_iter().map(|part| self.part(part)).collect()
    }

    fn part(&self, part: Part) -> Result<Part, TurnError> {
        let Part::File { mime, name, url } = part else { return Ok(part) };
        if let Some(path) = file_path(&url) {
            return Ok(text_part(&display_name(self.workspace, &path), &self.mention(&path)));
        }
        if !url.starts_with("data:") {
            return Err(TurnError::Attachment(format!("{name}: only files and data URLs can be attached")));
        }
        match mime.split('/').next().unwrap_or_default() {
            "text" => Ok(Part::File { mime, name, url }),
            "image" if self.model.attachment => Ok(Part::File { mime, name, url }),
            "image" => Err(TurnError::Attachment(format!("{} cannot read images; pick a model that can, or remove {name}", self.model.name))),
            _ => Err(TurnError::Attachment(format!("{name} ({mime}) cannot be sent to a model yet; attach it as text"))),
        }
    }

    /// The mentioned file's text, or a note saying why it was not read.
    fn mention(&self, path: &Path) -> String {
        let shown = display_name(self.workspace, path);
        if let Some(ask) = crate::tool::read_ask(self.workspace, path, "Read") {
            match self.engine.permissions.decide_now(self.session_id, self.policy, &ask) {
                Decision::Allow => {}
                Decision::Deny => return format!("[@{shown} was mentioned but a rule forbids reading it.]"),
                Decision::Ask => return format!("[@{shown} was mentioned but not read: {}. Use the read tool, which asks the user first.]", why(self.workspace, path)),
            }
        }
        read(path).map_or_else(|reason| format!("[@{shown} was mentioned but {reason}.]"), |text| format!("<file path=\"{shown}\">\n{text}\n</file>"))
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

/// A file's text within the mention bound, or a directory's entries; `Err` says why neither.
fn read(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|_| "it does not exist".to_string())?;
    if meta.is_dir() {
        return Ok(list(path));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("it could not be read ({e})"))?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("it is binary".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.len() <= MAX_MENTION_BYTES {
        return Ok(text.into_owned());
    }
    let cut = text[..text.floor_char_boundary(MAX_MENTION_BYTES)].rfind('\n').unwrap_or(0);
    let shown_lines = text[..cut].lines().count();
    Ok(format!("{}\n\n(cut after {shown_lines} lines; read with offset {} for the rest)", &text[..cut], shown_lines + 1))
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

fn text_part(name: &str, text: &str) -> Part {
    let url = format!("data:text/plain;base64,{}", base64::engine::general_purpose::STANDARD.encode(text));
    Part::File { mime: "text/plain".into(), name: name.into(), url }
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

/// A text data URL's content, for the request builder.
pub fn data_text(url: &str) -> Option<String> {
    let (header, data) = url.strip_prefix("data:")?.split_once(',')?;
    if header.ends_with(";base64") {
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    Some(percent_encoding::percent_decode_str(data).decode_utf8_lossy().into_owned())
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
        assert_eq!(data_text("data:text/plain,a%20b"), Some("a b".into()));
    }
}
