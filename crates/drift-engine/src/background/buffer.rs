//! What a background process printed: the newest part in memory for the model and the dock, and all of
//! it (up to a cap) in a log file the model can `read` or `grep`.

use std::io::Write;
use std::path::PathBuf;

use super::plain::Plain;
use crate::tool::spool::MAX_SPOOLED_BYTES;

/// How much of the newest output stays in memory.
const KEPT_BYTES: usize = 256 * 1024;
/// The most new output one read hands the model; older new output stays in the log.
pub(super) const READ_BYTES: usize = 32 * 1024;

pub(super) struct Buffer {
    /// The newest output; `start` is the offset of its first byte in everything printed.
    kept: Vec<u8>,
    start: u64,
    /// How far the model has read, as an offset in everything printed.
    read_to: u64,
    log: Option<std::fs::File>,
    pub(super) path: PathBuf,
    written: u64,
    /// Colors and cursor moves are taken out before anything is kept or logged.
    plain: Plain,
    /// The start of a character the last read split, held until the read that finishes it.
    partial: Vec<u8>,
}

/// New output since the model last looked.
pub struct Unread {
    pub text: String,
    /// Bytes printed since then that this read leaves out; they are in the log.
    pub skipped: u64,
}

impl Buffer {
    pub(super) fn new(path: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        let log = std::fs::File::create(&path).ok();

        Self {
            kept: Vec::new(),
            start: 0,
            read_to: 0,
            log,
            path,
            written: 0,
            plain: Plain::default(),
            partial: Vec::new(),
        }
    }

    /// Output the model already saw, as the command's result before it moved to the background.
    pub(super) fn seen(&mut self, bytes: &[u8]) -> String {
        let text = self.push(bytes);
        self.read_to = self.total();

        text
    }

    /// Keeps and logs what a process printed, without escape sequences; returns it as text.
    pub(super) fn push(&mut self, printed: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.partial);
        bytes.extend(self.plain.strip(printed));
        self.partial = bytes.split_off(whole_characters(&bytes));
        self.keep(&bytes);

        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Once the output has ended, a character it cut short is kept as U+FFFD rather than lost.
    pub(super) fn finish(&mut self) {
        let partial = std::mem::take(&mut self.partial);
        if !partial.is_empty() {
            self.keep(String::from_utf8_lossy(&partial).as_bytes());
        }
    }

    fn keep(&mut self, bytes: &[u8]) {
        if let Some(log) = self.log.as_mut().filter(|_| self.written < MAX_SPOOLED_BYTES) {
            let room = usize::try_from(MAX_SPOOLED_BYTES - self.written).unwrap_or(usize::MAX);
            let part = &bytes[..bytes.len().min(room)];
            if log.write_all(part).is_ok() {
                self.written += part.len() as u64;
            }
        }

        self.kept.extend_from_slice(bytes);
        let over = self.kept.len().saturating_sub(KEPT_BYTES);
        if over > 0 {
            self.kept.drain(..over);
            self.start += over as u64;
        }
    }

    pub(super) fn total(&self) -> u64 {
        self.start + self.kept.len() as u64
    }

    /// Everything printed since the last read, its newest [`READ_BYTES`] when there is more; marks it read.
    pub(super) fn unread(&mut self) -> Unread {
        let total = self.total();
        let from = self
            .read_to
            .max(total.saturating_sub(READ_BYTES as u64))
            .max(self.start);
        let skipped = from - self.read_to;
        let offset = usize::try_from(from - self.start).unwrap_or(0);
        self.read_to = total;

        Unread {
            text: String::from_utf8_lossy(&self.kept[offset..]).into_owned(),
            skipped,
        }
    }

    /// Whether output the model has not read yet holds a line `matches` accepts.
    pub(super) fn unread_matches(&self, matches: &dyn Fn(&str) -> bool) -> bool {
        let offset = usize::try_from(self.read_to.saturating_sub(self.start)).unwrap_or(0);
        let unread = String::from_utf8_lossy(&self.kept[offset.min(self.kept.len())..]).into_owned();

        unread.lines().any(matches)
    }

    /// Lines of the output still in memory that `matches` accepts, the newest `limit` of them.
    pub(super) fn search(&self, matches: &dyn Fn(&str) -> bool, limit: usize) -> Vec<String> {
        let text = String::from_utf8_lossy(&self.kept);
        let found: Vec<&str> = text.lines().filter(|line| matches(line)).collect();

        found[found.len().saturating_sub(limit)..]
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// The last line with something on it, for the dock.
    pub(super) fn last_line(&self) -> Option<String> {
        let tail = &self.kept[self.kept.len().saturating_sub(4096)..];
        String::from_utf8_lossy(tail)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(200).collect())
    }

    /// The newest `bytes` of output, for a result that says how a process ended.
    pub(super) fn tail(&self, bytes: usize) -> String {
        String::from_utf8_lossy(&self.kept[self.kept.len().saturating_sub(bytes)..]).into_owned()
    }
}

/// Hosts a dev server on this machine announces itself on.
const LOCAL_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"];

/// How many leading bytes end on a whole character: all but a character cut short at the very end.
fn whole_characters(bytes: &[u8]) -> usize {
    let mut from = 0;
    loop {
        match std::str::from_utf8(&bytes[from..]) {
            Ok(_) => return bytes.len(),
            Err(error) => match error.error_len() {
                None => return from + error.valid_up_to(),
                Some(invalid) => from += error.valid_up_to() + invalid,
            },
        }
    }
}

/// The address a local server prints when it is ready: a local host and a real port. Addresses a test or a log
/// merely mentions (example.com, a deliberately refused port) are not one.
pub(super) fn url_in(line: &str) -> Option<String> {
    line.match_indices("http")
        .filter_map(|(start, _)| address_at(&line[start..]))
        .find(|url| is_local_server(url))
}

fn address_at(text: &str) -> Option<String> {
    if !text.starts_with("http://") && !text.starts_with("https://") {
        return None;
    }
    let url: String = text
        .chars()
        .take_while(|character| !character.is_whitespace() && !matches!(character, '"' | '\'' | '<' | '>' | ')'))
        .collect();

    Some(url.trim_end_matches(['.', ',', ';']).to_string())
}

fn is_local_server(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or_default();
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };

    let port: u16 = port.parse().unwrap_or(0);
    LOCAL_HOSTS.contains(&host) && (port == 80 || port >= 1024)
}
