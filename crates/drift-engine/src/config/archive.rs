//! Archives fetched over https or through a registry source and unpacked under the user's config:
//! GitHub serves tar.gz, Azure DevOps serves zip.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::sources::{Fetcher, Location, RegistryKind, RegistrySource};

/// The most an archive may unpack to, so a small download cannot fill the disk.
const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("an archive is fetched over https only")]
    RequiresHttps,
    #[error("the archive unpacks to more than {} MB", MAX_UNPACKED_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("could not {operation} {}: {source}", path.display())]
    File {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("not a zip archive: {0}")]
    InvalidZip(zip::result::ZipError),
    #[error("not a tar.gz archive: {0}")]
    InvalidTar(std::io::Error),
    #[error("could not read the archive: {0}")]
    Entry(std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Source(#[from] super::sources::SourceError),
}

pub(super) fn file_error(operation: &'static str, path: &Path, source: std::io::Error) -> ArchiveError {
    ArchiveError::File {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

/// The archive's bytes: through the source with its token and trust, or over plain https for Drift's own
/// registry. An empty `url` with a repository source means the source's own repository.
pub(super) async fn fetch(
    fetcher: &Fetcher,
    source: Option<&RegistrySource>,
    url: &str,
    limit: usize,
) -> Result<Vec<u8>, ArchiveError> {
    if let Some(source) = source {
        let token = fetcher.token(source);
        let location = if url.trim().is_empty() {
            source.archive(token.as_deref())?
        } else {
            source.file(url, token.as_deref())?
        };
        return Ok(fetcher.read(source, location, limit).await?);
    }

    if !url.starts_with("https://") {
        return Err(ArchiveError::RequiresHttps);
    }
    let drift = RegistrySource {
        id: String::new(),
        name: "Drift".into(),
        kind: RegistryKind::Plugins,
        source: Default::default(),
        url: url.into(),
        r#ref: String::new(),
        path: String::new(),
        has_token: false,
        allow_http: false,
        ca_pem: None,
    };
    let location = Location::Http {
        url: url.into(),
        headers: Vec::new(),
    };

    Ok(fetcher.read(&drift, location, limit).await?)
}

/// What to unpack: where it will go, which entries (by their path inside the archive, mapped to where they land), and
/// the largest file kept; a larger one is skipped when set, kept otherwise.
pub(super) struct Unpack<'a> {
    pub(super) into: &'a Path,
    pub(super) keep: &'a dyn Fn(&Path) -> Option<PathBuf>,
    pub(super) max_file: Option<u64>,
}

/// An archive unpacked beside where it goes. Nothing there changes until [`Staged::commit`], so a failed or
/// rejected install leaves what was installed before; dropped uncommitted, it is removed.
pub(super) struct Staged {
    dir: PathBuf,
    /// Where each kept entry landed, relative to the folder.
    pub(super) paths: Vec<PathBuf>,
}

/// One swap at a time, so two installs of the same folder never interleave their renames.
static SWAP: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Staged {
    /// Swaps the unpacked folder in for `into`, whatever was there before going.
    pub(super) fn commit(self, into: &Path) -> Result<(), ArchiveError> {
        let _swap = SWAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let old = sibling(into, "old");
        // A swap cut short between its renames left the only copy of the previous install here.
        if old.exists() && !into.exists() {
            std::fs::rename(&old, into).map_err(|error| file_error("restore", into, error))?;
        }
        let _ = std::fs::remove_dir_all(&old);
        if into.exists() {
            std::fs::rename(into, &old).map_err(|error| file_error("replace", into, error))?;
        }
        if let Err(error) = std::fs::rename(&self.dir, into) {
            let _ = std::fs::rename(&old, into);
            return Err(file_error("replace", into, error));
        }
        let _ = std::fs::remove_dir_all(&old);

        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A hidden folder beside `into` for one stage of a swap.
fn sibling(into: &Path, stage: &str) -> PathBuf {
    let name = into
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    into.with_file_name(format!(".{name}.{stage}"))
}

/// Unpacks the kept entries beside `plan.into`; nothing climbs out, and `plan.into` is untouched until committed.
/// Each attempt stages in its own folder, so a concurrent install of the same folder never writes into it.
pub(super) fn unpack(bytes: &[u8], plan: &Unpack) -> Result<Staged, ArchiveError> {
    let dir = sibling(plan.into, &format!("partial-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&dir).map_err(|error| file_error("create", &dir, error))?;
    let mut staged = Staged { dir, paths: Vec::new() };

    let mut written = Writer::under(&staged.dir);
    if bytes.starts_with(b"PK") {
        unpack_zip(bytes, plan, &mut written)?;
    } else {
        unpack_tar(bytes, plan, &mut written)?;
    }
    staged.paths = written.paths;

    Ok(staged)
}

/// Azure's zip has no top-level folder, so paths are taken as they are.
fn unpack_zip(bytes: &[u8], plan: &Unpack, written: &mut Writer) -> Result<(), ArchiveError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(ArchiveError::InvalidZip)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() || plan.max_file.is_some_and(|max| entry.size() > max) {
            continue;
        }
        let Some(relative) = entry.enclosed_name().and_then(|name| (plan.keep)(&name)) else {
            continue;
        };
        let mode = entry.unix_mode();
        written.write(relative, &mut entry, mode)?;
    }

    Ok(())
}

/// GitHub's tar.gz holds everything under one top folder, which is dropped.
fn unpack_tar(bytes: &[u8], plan: &Unpack, written: &mut Writer) -> Result<(), ArchiveError> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let entries = archive.entries().map_err(ArchiveError::InvalidTar)?;

    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Entry)?;
        let size = entry.size();
        if !entry.header().entry_type().is_file() || plan.max_file.is_some_and(|max| size > max) {
            continue;
        }
        let path = entry.path().map_err(ArchiveError::Entry)?.into_owned();
        let Some(relative) = without_top_folder(&path).and_then(|inner| (plan.keep)(&inner)) else {
            continue;
        };
        let mode = entry.header().mode().ok();
        written.write(relative, &mut entry, mode)?;
    }

    Ok(())
}

/// An archive path without its top folder; none for the folder itself or a path that climbs.
pub(super) fn without_top_folder(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    parts.next()?;
    let relative = parts.as_path().to_path_buf();

    safe(&relative).then_some(relative)
}

/// Only plain names: nothing absolute and nothing that climbs.
pub(super) fn safe(relative: &Path) -> bool {
    !relative.as_os_str().is_empty() && relative.components().all(|part| matches!(part, Component::Normal(_)))
}

struct Writer<'a> {
    into: &'a Path,
    paths: Vec<PathBuf>,
    total: u64,
}

impl<'a> Writer<'a> {
    fn under(into: &'a Path) -> Self {
        Self {
            into,
            paths: Vec::new(),
            total: 0,
        }
    }

    /// Streams the entry to disk, counting the bytes it really holds rather than what its header claims.
    fn write(&mut self, relative: PathBuf, entry: &mut impl Read, mode: Option<u32>) -> Result<(), ArchiveError> {
        if !safe(&relative) {
            return Ok(());
        }

        let target = self.into.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| file_error("create", parent, error))?;
        }
        let mut file = std::fs::File::create(&target).map_err(|error| file_error("write", &target, error))?;
        let room = MAX_UNPACKED_BYTES - self.total;
        let copied = std::io::copy(&mut entry.take(room + 1), &mut file).map_err(ArchiveError::Entry)?;
        self.total += copied;
        if copied > room {
            return Err(ArchiveError::TooLarge);
        }
        keep_executable(&target, mode);

        self.paths.push(relative);
        Ok(())
    }
}

/// A program keeps its executable bit, so a bundle's `bin/` runs as shipped.
#[cfg(unix)]
fn keep_executable(target: &Path, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode.filter(|mode| mode & 0o111 != 0) {
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode & 0o755));
    }
}

#[cfg(not(unix))]
fn keep_executable(_target: &Path, _mode: Option<u32>) {}
