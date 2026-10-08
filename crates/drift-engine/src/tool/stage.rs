//! Whole-file replacement: written beside the file, then swapped in, so a failed write never cuts it short.

use std::io;
use std::path::Path;

use crate::store::{StagedReplacement, Store};

/// Writes `bytes` to `path` through engine-owned siblings recorded in the store before they exist.
pub(crate) async fn replace(store: &Store, path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let original = tokio::fs::metadata(path).await.ok();
    if original.as_ref().is_some_and(|meta| meta.permissions().readonly()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the file is read-only"));
    }

    let mut pair = beside(path);
    store.record_replacement(&pair).map_err(io::Error::other)?;
    let swapped = swap(&pair, bytes, original).await;
    if swapped.is_ok() {
        // From here a leftover backup is old content: recovery removes it and never brings it back.
        pair.swapped = store.mark_swapped(&pair.staged).is_ok();
    }

    let settled = {
        let pair = pair.clone();
        tokio::task::spawn_blocking(move || settle(&pair))
            .await
            .map_err(io::Error::other)?
    };
    // An unsettled pair keeps its record, so the next start recovers it.
    if settled.is_ok() {
        let _ = store.forget_replacements(&[&pair.staged]);
    }

    match (swapped, settled) {
        (Err(error), Err(stuck)) => Err(io::Error::new(
            error.kind(),
            format!(
                "{error}; the original could not be put back ({stuck}) and is kept at {}",
                pair.backup
            ),
        )),
        (swapped, _) => swapped,
    }
}

/// At startup, before any tool runs: puts back originals a crash left in a backup and removes the rest of each recorded pair.
pub(crate) fn recover_leftovers(store: &Store) -> usize {
    let mut settled = Vec::new();
    for pair in store.replacements().unwrap_or_default() {
        // A record whose names the engine would not have given is never acted on, only dropped.
        if !is_pair(&pair) || settle(&pair).is_ok() {
            settled.push(pair.staged);
        }
    }

    let names: Vec<&str> = settled.iter().map(String::as_str).collect();
    let _ = store.forget_replacements(&names);
    settled.len()
}

/// Moves a backup back over a missing destination, keeping both siblings if that fails, then removes them.
fn settle(pair: &StagedReplacement) -> io::Result<()> {
    let (destination, staged, backup) = (
        Path::new(&pair.destination),
        Path::new(&pair.staged),
        Path::new(&pair.backup),
    );

    if !pair.swapped && !present(destination)? && present(backup)? {
        #[cfg(test)]
        tests::fault(tests::Fault::Restore, destination)?;
        std::fs::rename(backup, destination)?;
    }

    remove_if_present(staged)?;
    #[cfg(test)]
    tests::fault(tests::Fault::RemoveBackup, destination)?;
    remove_if_present(backup)
}

fn present(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn beside(path: &Path) -> StagedReplacement {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tag = crate::random_hex(4);
    let sibling = |extension: &str| {
        path.with_file_name(format!(".{name}.drift-{tag}.{extension}"))
            .to_string_lossy()
            .into_owned()
    };

    StagedReplacement {
        destination: path.to_string_lossy().into_owned(),
        staged: sibling("tmp"),
        backup: sibling("bak"),
        swapped: false,
    }
}

/// Exactly what [`beside`] would make for this destination, with one tag.
fn is_pair(pair: &StagedReplacement) -> bool {
    let destination = Path::new(&pair.destination);
    let Some(name) = destination.file_name().map(|name| name.to_string_lossy().into_owned()) else {
        return false;
    };
    let Some(tag) = Path::new(&pair.staged).file_name().and_then(|staged| {
        staged
            .to_string_lossy()
            .strip_prefix(&format!(".{name}.drift-"))
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .map(str::to_owned)
    }) else {
        return false;
    };

    let expected = |extension: &str| {
        destination
            .with_file_name(format!(".{name}.drift-{tag}.{extension}"))
            .to_string_lossy()
            .into_owned()
    };
    tag.len() == 8
        && tag.chars().all(|digit| digit.is_ascii_hexdigit())
        && pair.staged == expected("tmp")
        && pair.backup == expected("bak")
}

async fn swap(pair: &StagedReplacement, bytes: &[u8], original: Option<std::fs::Metadata>) -> io::Result<()> {
    let (staged, backup, path) = (
        Path::new(&pair.staged),
        Path::new(&pair.backup),
        Path::new(&pair.destination),
    );

    tokio::fs::write(staged, bytes).await?;
    #[cfg(test)]
    tests::fault(tests::Fault::AfterStaging, path)?;

    match original {
        Some(meta) => replace_existing(staged, backup, path, meta).await,
        None => tokio::fs::rename(staged, path).await,
    }
}

/// Unix keeps the original's mode; owner, group, extended attributes and ACLs are the new file's own.
#[cfg(not(windows))]
async fn replace_existing(staged: &Path, _backup: &Path, path: &Path, meta: std::fs::Metadata) -> io::Result<()> {
    tokio::fs::set_permissions(staged, meta.permissions()).await?;
    tokio::fs::rename(staged, path).await
}

/// `ReplaceFileW` without its ignore flags: the original's ACL and attributes carry over or the call fails.
#[cfg(windows)]
async fn replace_existing(staged: &Path, backup: &Path, path: &Path, _meta: std::fs::Metadata) -> io::Result<()> {
    let (staged, backup, path) = (staged.to_path_buf(), backup.to_path_buf(), path.to_path_buf());
    tokio::task::spawn_blocking(move || windows::replace_file(&path, &staged, &backup))
        .await
        .map_err(io::Error::other)?
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(windows)]
mod windows;
