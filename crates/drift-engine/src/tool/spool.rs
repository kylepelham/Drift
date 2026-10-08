//! Tool output within fixed memory: all of it while it is small, then only its start and end in
//! memory with the whole of it (up to a cap) in a file the model can read back.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;

pub const HEAD_BYTES: usize = 16 * 1024;
pub const TAIL_BYTES: usize = 16 * 1024;
/// Past this, the file stops growing; the start and end are still kept.
pub const MAX_SPOOLED_BYTES: u64 = 64 * 1024 * 1024;
/// The most any tool result puts in front of the model. Tools that page (read) stay under it by
/// themselves; anything else past it keeps its start and end, with the whole of it in a file.
pub const MAX_RESULT_BYTES: usize = 64 * 1024;

/// A tool result within [`MAX_RESULT_BYTES`], and the file holding all of it when it was cut.
pub fn bound(text: String, path: PathBuf) -> (String, Option<PathBuf>) {
    if text.len() <= MAX_RESULT_BYTES {
        return (text, None);
    }
    let mut spool = Spool::new(Some(path));
    spool.push(text.as_bytes());
    let kept = spool.finish();
    (kept.text, kept.file)
}

pub struct Spool {
    /// Everything so far, until it outgrows head and tail together.
    small: Vec<u8>,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: u64,
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
    written: u64,
}

/// What a spool kept: text for the model, how much there was, and where all of it is.
pub struct Spooled {
    pub text: String,
    pub total: u64,
    /// Set when the text is only the start and end.
    pub file: Option<PathBuf>,
}

impl Spool {
    /// `path` is where the whole output goes once it no longer fits; `None` keeps only start and end.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { small: Vec::new(), head: Vec::new(), tail: VecDeque::new(), total: 0, path, file: None, written: 0 }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        if self.head.is_empty() && self.small.len() + bytes.len() <= HEAD_BYTES + TAIL_BYTES {
            self.small.extend_from_slice(bytes);
            return;
        }
        if self.head.is_empty() {
            let small = std::mem::take(&mut self.small);
            self.open_file();
            self.keep(&small);
        }
        self.keep(bytes);
    }

    fn open_file(&mut self) {
        let Some(path) = &self.path else { return };
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
        self.file = std::fs::File::create(path).ok();
    }

    fn keep(&mut self, bytes: &[u8]) {
        let into_head = (HEAD_BYTES - self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..into_head]);
        self.tail.extend(&bytes[into_head..]);
        let excess = self.tail.len().saturating_sub(TAIL_BYTES);
        self.tail.drain(..excess);
        let room = MAX_SPOOLED_BYTES.saturating_sub(self.written).min(bytes.len() as u64) as usize;
        if let Some(file) = &mut self.file
            && room > 0 && file.write_all(&bytes[..room]).is_ok() {
                self.written += room as u64;
            }
    }

    /// The last `max` bytes so far, for showing while the command still runs; cut text starts `...`.
    pub fn recent(&self, max: usize) -> String {
        let end: Vec<u8> = if self.head.is_empty() { self.small.clone() } else { self.tail.iter().copied().collect() };
        let from = end.len().saturating_sub(max);
        let text = String::from_utf8_lossy(&end[from..]).into_owned();
        if from == 0 && self.head.is_empty() {
            text
        } else {
            format!("...\n{text}")
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn finish(mut self) -> Spooled {
        if self.head.is_empty() {
            return Spooled { text: String::from_utf8_lossy(&self.small).into_owned(), total: self.total, file: None };
        }
        let file = self.file.take().and_then(|_| self.path.clone());
        let omitted = self.total - (self.head.len() + self.tail.len()) as u64;
        let whole = match &file {
            Some(path) if self.written == self.total => format!("the whole output is in {}", path.display()),
            Some(path) => format!("the first {} MB are in {}", MAX_SPOOLED_BYTES / 1024 / 1024, path.display()),
            None => "it was not kept".into(),
        };
        let tail: Vec<u8> = self.tail.into_iter().collect();
        let text = format!("{}\n\n... {omitted} bytes omitted; {whole} ...\n\n{}", String::from_utf8_lossy(&self.head), String::from_utf8_lossy(&tail));
        Spooled { text, total: self.total, file }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_stays_whole_and_in_memory() {
        let dir = std::env::temp_dir().join(format!("drift-spool-{}", crate::random_hex(4)));
        let mut spool = Spool::new(Some(dir.join("out.log")));
        spool.push(b"hello ");
        spool.push(b"world");
        let kept = spool.finish();
        assert_eq!((kept.text.as_str(), kept.total, kept.file), ("hello world", 11, None));
        assert!(!dir.exists(), "no file for output that fits");
    }

    #[test]
    fn large_output_keeps_start_and_end_in_memory_and_all_of_it_on_disk() {
        let dir = std::env::temp_dir().join(format!("drift-spool-{}", crate::random_hex(4)));
        let path = dir.join("out.log");
        let mut spool = Spool::new(Some(path.clone()));
        let chunk: Vec<u8> = (0..4096u32).map(|i| b'a' + (i % 26) as u8).collect();
        spool.push(b"FIRST LINE\n");
        for _ in 0..2_500 {
            spool.push(&chunk);
        }
        spool.push(b"\nLAST LINE");
        assert!(spool.head.len() == HEAD_BYTES && spool.tail.len() == TAIL_BYTES && spool.small.is_empty(), "memory stays bounded");
        let kept = spool.finish();
        assert_eq!(kept.total, 11 + 2_500 * 4096 + 10);
        assert!(kept.text.starts_with("FIRST LINE") && kept.text.ends_with("LAST LINE"));
        assert!(kept.text.len() < HEAD_BYTES + TAIL_BYTES + 200);
        assert!(kept.text.contains("the whole output is in"), "{}", &kept.text[HEAD_BYTES..HEAD_BYTES + 200]);
        assert_eq!(std::fs::metadata(kept.file.unwrap()).unwrap().len(), kept.total);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_result_past_the_limit_is_cut_to_its_ends_and_kept_whole_on_disk() {
        let dir = std::env::temp_dir().join(format!("drift-bound-{}", crate::random_hex(4)));
        let (small, none) = bound("fine".into(), dir.join("small.log"));
        assert_eq!((small.as_str(), none), ("fine", None));
        let big = format!("START{}END", "m".repeat(MAX_RESULT_BYTES * 3));
        let (text, file) = bound(big.clone(), dir.join("big.log"));
        assert!(text.len() <= HEAD_BYTES + TAIL_BYTES + 200 && text.starts_with("START") && text.ends_with("END"));
        assert_eq!(std::fs::read_to_string(file.unwrap()).unwrap(), big);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn without_a_file_the_middle_is_simply_dropped() {
        let mut spool = Spool::new(None);
        spool.push(&vec![b'x'; HEAD_BYTES * 4]);
        let kept = spool.finish();
        assert!(kept.file.is_none() && kept.text.contains("it was not kept"));
    }
}
