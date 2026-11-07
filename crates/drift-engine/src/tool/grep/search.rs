//! The walk behind `grep`: files on several threads, each searched whole, until the time budget runs out,
//! enough is found to show, or a Stop.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use grep::regex::RegexMatcher;
use grep::searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::{DirEntry, WalkState};
use tokio_util::sync::CancellationToken;

use super::{LARGE_FILE_BYTES, Mode, Options};
use crate::tool::FileGlob;
use crate::tool::sensitive::is_sensitive;

/// One line shown from a file: its number, whether it matched or is context, and whether lines were skipped before it.
pub(super) struct Line {
    pub number: u64,
    pub matched: bool,
    pub gap: bool,
    pub text: String,
}

/// What one file gave: its matching and context lines, or only how many lines matched.
pub(super) struct FileHits {
    pub path: PathBuf,
    pub count: usize,
    pub lines: Vec<Line>,
}

pub(super) struct Search<'a> {
    pub root: &'a Path,
    pub matcher: &'a RegexMatcher,
    pub include: Option<&'a FileGlob>,
    pub options: &'a Options,
    pub stop: &'a CancellationToken,
    pub allowed: Option<&'a (dyn Fn(&Path) -> bool + Sync)>,
    pub deadline: Instant,
    /// Entries to collect before stopping early; none walks everything.
    pub enough: Option<usize>,
}

#[derive(Default)]
pub(super) struct Tally {
    pub withheld: AtomicUsize,
    pub restricted: AtomicUsize,
    pub unreadable: AtomicUsize,
    pub large: AtomicUsize,
    /// Entries collected so far, as [`Options::mode`] counts them.
    pub entries: AtomicUsize,
    pub out_of_time: AtomicBool,
    pub enough: AtomicBool,
    pub hits: Mutex<Vec<FileHits>>,
}

impl Search<'_> {
    pub(super) fn run(&self, walk: &ignore::WalkBuilder, tally: &Tally) {
        walk.build_parallel().run(|| {
            let mut searcher = self.searcher();
            Box::new(move |entry| self.visit(&mut searcher, entry, tally))
        });
    }

    fn searcher(&self) -> Searcher {
        let content = self.options.mode == Mode::Content;
        SearcherBuilder::new()
            .line_number(true)
            .binary_detection(BinaryDetection::quit(0))
            .multi_line(self.options.multiline)
            .before_context(if content { self.options.before } else { 0 })
            .after_context(if content { self.options.after } else { 0 })
            .build()
    }

    fn visit(&self, searcher: &mut Searcher, entry: Result<DirEntry, ignore::Error>, tally: &Tally) -> WalkState {
        if self.over(tally) {
            return WalkState::Quit;
        }
        let Ok(entry) = entry else {
            tally.unreadable.fetch_add(1, Ordering::Relaxed);
            return WalkState::Continue;
        };
        if self.searchable(&entry, tally) {
            self.search_file(searcher, entry.path(), tally);
        }

        WalkState::Continue
    }

    /// Whether to stop: a Stop, the time budget spent, or enough found.
    fn over(&self, tally: &Tally) -> bool {
        if Instant::now() >= self.deadline {
            tally.out_of_time.store(true, Ordering::Relaxed);
        }

        self.stop.is_cancelled() || tally.out_of_time.load(Ordering::Relaxed) || tally.enough.load(Ordering::Relaxed)
    }

    /// A file to search: inside the include glob, not a secret, not too large, and readable by the session's rules.
    /// The file the search names is searched whatever it is, since naming it already asked.
    fn searchable(&self, entry: &DirEntry, tally: &Tally) -> bool {
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || self.include.is_some_and(|glob| !glob.matches(path))
        {
            return false;
        }
        if path == self.root {
            return true;
        }
        let skipped = if is_sensitive(path) {
            &tally.withheld
        } else if entry.metadata().is_ok_and(|metadata| metadata.len() > LARGE_FILE_BYTES) {
            &tally.large
        } else if self.allowed.is_some_and(|allowed| !allowed(path)) {
            &tally.restricted
        } else {
            return true;
        };
        skipped.fetch_add(1, Ordering::Relaxed);

        false
    }

    fn search_file(&self, searcher: &mut Searcher, path: &Path, tally: &Tally) {
        let mut sink = Collect {
            search: self,
            tally,
            hits: FileHits {
                path: path.to_path_buf(),
                count: 0,
                lines: Vec::new(),
            },
            gap: false,
        };
        if searcher.search_path(self.matcher, path, &mut sink).is_err() {
            tally.unreadable.fetch_add(1, Ordering::Relaxed);
        }
        if sink.hits.count > 0 {
            tally.hits.lock().unwrap().push(sink.hits);
        }
    }
}

/// Gathers one file's matches as [`Options::mode`] wants them.
struct Collect<'a> {
    search: &'a Search<'a>,
    tally: &'a Tally,
    hits: FileHits,
    gap: bool,
}

impl Collect<'_> {
    fn push(&mut self, number: u64, matched: bool, bytes: &[u8]) -> Result<(), std::io::Error> {
        let text = std::str::from_utf8(bytes).map_err(std::io::Error::other)?;
        for (index, line) in text.lines().enumerate() {
            self.hits.lines.push(Line {
                number: number + index as u64,
                matched,
                gap: std::mem::take(&mut self.gap),
                text: super::clip(line.trim_end()),
            });
            self.counted(1);
        }

        Ok(())
    }

    /// Counts collected entries; once the search has enough, every file's search ends.
    fn counted(&self, entries: usize) {
        let collected = self.tally.entries.fetch_add(entries, Ordering::Relaxed) + entries;
        if self.search.enough.is_some_and(|enough| collected >= enough) {
            self.tally.enough.store(true, Ordering::Relaxed);
        }
    }

    fn keeps_going(&self) -> bool {
        !self.search.stop.is_cancelled()
            && !self.tally.enough.load(Ordering::Relaxed)
            && Instant::now() < self.search.deadline
    }
}

impl Sink for Collect<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, found: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.hits.count += 1;
        match self.search.options.mode {
            Mode::Content => self.push(found.line_number().unwrap_or(0), true, found.bytes())?,
            Mode::Files => {
                self.counted(1);
                return Ok(false);
            }
            Mode::Count => {}
        }

        Ok(self.keeps_going())
    }

    fn context(&mut self, _: &Searcher, context: &SinkContext<'_>) -> Result<bool, Self::Error> {
        self.push(context.line_number().unwrap_or(0), false, context.bytes())?;

        Ok(self.keeps_going())
    }

    fn context_break(&mut self, _: &Searcher) -> Result<bool, Self::Error> {
        self.gap = true;

        Ok(true)
    }

    fn finish(&mut self, _: &Searcher, _: &grep::searcher::SinkFinish) -> Result<(), Self::Error> {
        if self.search.options.mode == Mode::Count && self.hits.count > 0 {
            self.counted(1);
        }

        Ok(())
    }
}
