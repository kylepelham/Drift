use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::session::review::git_text;
use crate::session::snapshot::plain_git;

/// Files counted line by line; the rest are only reported as not measured.
const MAX_MEASURED: usize = 20_000;
const MAX_BYTES: u64 = 1024 * 1024;
/// A folder with more lines than this is split into its children, at most this deep.
const SPLIT_LINES: usize = 4_000;
const MAX_DEPTH: usize = 8;
const MAX_CHURN: usize = 12;
const MAX_LARGEST: usize = 8;
const LOCKFILES: [&str; 6] = [
    "Cargo.lock",
    "bun.lock",
    "bun.lockb",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
];
const SKIPPED_ENDINGS: [&str; 6] = [".md", ".mdx", ".txt", ".svg", ".min.js", ".map"];

/// What `/audit` looks at.
#[derive(Debug, PartialEq)]
pub(super) enum Surveyed {
    NotRepo,
    /// A repository whose target holds no file worth counting.
    Empty,
    Files(Survey),
}

#[derive(Debug, Default, PartialEq)]
pub(super) struct Survey {
    /// By path, relative to the workspace with `/` separators.
    pub files: Vec<Counted>,
    /// Files past [`MAX_MEASURED`], never read.
    pub unmeasured: usize,
    /// The files changed in most commits of the last 90 days, with their commit counts.
    pub churn: Vec<(String, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Counted {
    pub path: String,
    pub lines: usize,
}

/// A folder, or the files sitting directly in a folder that was split.
#[derive(Debug, PartialEq)]
pub(super) struct Area {
    /// Empty for the workspace itself.
    pub path: String,
    pub direct: bool,
    pub lines: usize,
}

impl Area {
    pub(super) fn label(&self) -> String {
        match (self.direct, self.path.is_empty()) {
            (true, true) => "files at the top level".into(),
            (true, false) => format!("files directly in `{}/`", self.path),
            (false, _) => format!("`{}/`", self.path),
        }
    }
}

impl Survey {
    pub(super) fn lines(&self) -> usize {
        self.files.iter().map(|file| file.lines).sum()
    }

    pub(super) fn largest(&self) -> Vec<&Counted> {
        let mut files: Vec<&Counted> = self.files.iter().collect();
        files.sort_by(|left, right| right.lines.cmp(&left.lines).then_with(|| left.path.cmp(&right.path)));
        files.truncate(MAX_LARGEST);
        files
    }

    /// Folders from the files' common one down, any past [`SPLIT_LINES`] split into its children.
    pub(super) fn areas(&self) -> Vec<Area> {
        let files: Vec<&Counted> = self.files.iter().collect();
        let mut areas = Vec::new();
        split(&common_folder(&self.files), &files, 0, &mut areas);
        areas
    }
}

fn common_folder(files: &[Counted]) -> String {
    let folder = |path: &str| -> Vec<String> {
        let parent = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        parent.split('/').map(str::to_string).collect()
    };
    let Some(first) = files.first() else {
        return String::new();
    };
    let mut common = folder(&first.path);
    for file in &files[1..] {
        let shared = common
            .iter()
            .zip(folder(&file.path))
            .take_while(|(left, right)| **left == *right)
            .count();
        common.truncate(shared);
    }

    common.join("/")
}

fn split(folder: &str, files: &[&Counted], depth: usize, areas: &mut Vec<Area>) {
    let mut children: BTreeMap<&str, Vec<&Counted>> = BTreeMap::new();
    let mut direct: Option<usize> = None;
    for file in files {
        let rest = file
            .path
            .strip_prefix(folder)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(&file.path);
        match rest.split_once('/') {
            Some((child, _)) => children.entry(child).or_default().push(file),
            None => *direct.get_or_insert(0) += file.lines,
        }
    }

    for (child, inner) in children {
        let path = if folder.is_empty() {
            child.to_string()
        } else {
            format!("{folder}/{child}")
        };
        let lines = inner.iter().map(|file| file.lines).sum();
        if lines > SPLIT_LINES && depth < MAX_DEPTH {
            split(&path, &inner, depth + 1, areas);
        } else {
            areas.push(Area {
                path,
                direct: false,
                lines,
            });
        }
    }
    if let Some(lines) = direct {
        areas.push(Area {
            path: folder.to_string(),
            direct: true,
            lines,
        });
    }
}

/// `count` parts of about equal lines: the largest area first, each to the part with the fewest lines so far.
pub(super) fn balance(areas: &[Area], count: usize) -> Vec<(usize, Vec<&Area>)> {
    let mut sorted: Vec<&Area> = areas.iter().collect();
    sorted.sort_by(|left, right| right.lines.cmp(&left.lines).then_with(|| left.path.cmp(&right.path)));
    let mut parts: Vec<(usize, Vec<&Area>)> = vec![(0, Vec::new()); count.max(1)];
    for area in sorted {
        if let Some(lightest) = parts.iter_mut().min_by_key(|(lines, _)| *lines) {
            lightest.0 += area.lines;
            lightest.1.push(area);
        }
    }

    parts
}

/// The files of `target`, or of the whole workspace, counted, with the ones changed most lately.
pub(super) async fn run(workspace: &Path, target: Option<&str>) -> Surveyed {
    if git_text(workspace, &["rev-parse", "--is-inside-work-tree"])
        .await
        .as_deref()
        != Some("true")
    {
        return Surveyed::NotRepo;
    }

    let pathspec = target.unwrap_or(".");
    let listed = plain_git(
        workspace,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            pathspec,
        ],
        None,
    )
    .await
    .unwrap_or_default();
    let mut paths: Vec<String> = names(&listed).filter(|path| counted_name(path)).collect();
    paths.sort();
    paths.dedup();

    let root = workspace.to_path_buf();
    let (files, unmeasured) = tokio::task::spawn_blocking(move || measure(&root, paths))
        .await
        .unwrap_or_default();
    if files.is_empty() {
        return Surveyed::Empty;
    }
    let churn = churn(workspace, pathspec, &files).await;

    Surveyed::Files(Survey {
        files,
        unmeasured,
        churn,
    })
}

fn names(listed: &[u8]) -> impl Iterator<Item = String> + '_ {
    listed
        .split(|byte| *byte == 0)
        .map(|name| String::from_utf8_lossy(name).trim_matches('\n').to_string())
        .filter(|name| !name.is_empty())
}

/// Lockfiles, prose, vector images, minified scripts and source maps are not code to review.
fn counted_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();

    !LOCKFILES.contains(&name) && !SKIPPED_ENDINGS.iter().any(|ending| lower.ends_with(ending))
}

/// Up to [`MAX_MEASURED`] files counted, and how many paths were left untried once it was reached.
fn measure(root: &Path, paths: Vec<String>) -> (Vec<Counted>, usize) {
    let mut counted = Vec::new();
    let mut paths = paths.into_iter();
    for path in paths.by_ref() {
        counted.extend(count(root, path));
        if counted.len() == MAX_MEASURED {
            break;
        }
    }

    (counted, paths.len())
}

/// The file's lines; `None` when it is missing, a link, past [`MAX_BYTES`], binary or empty.
fn count(root: &Path, path: String) -> Option<Counted> {
    let full = root.join(&path);
    let small = std::fs::symlink_metadata(&full).is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_BYTES);
    let bytes = small.then(|| std::fs::read(&full).ok()).flatten()?;
    if bytes.contains(&0) {
        return None;
    }
    let breaks = bytes.iter().filter(|byte| **byte == b'\n').count();
    let lines = breaks + usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));

    (lines > 0).then_some(Counted { path, lines })
}

/// The surveyed files changed in most commits of the last 90 days.
async fn churn(workspace: &Path, pathspec: &str, files: &[Counted]) -> Vec<(String, usize)> {
    let log = plain_git(
        workspace,
        &[
            "log",
            "--since=90.days.ago",
            "--name-only",
            "--relative",
            "-z",
            "--format=",
            "--",
            pathspec,
        ],
        None,
    )
    .await
    .unwrap_or_default();
    let known: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for name in names(&log).filter(|name| known.contains(name.as_str())) {
        *counts.entry(name).or_default() += 1;
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked.truncate(MAX_CHURN);
    ranked
}
