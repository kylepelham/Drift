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
    let pair = beside(path);
    store.record_replacement(&pair).map_err(io::Error::other)?;
    let swapped = swap(&pair, bytes, original).await;
    let settled = {
        let pair = pair.clone();
        tokio::task::spawn_blocking(move || settle(&pair)).await.map_err(io::Error::other)?
    };
    // An unsettled pair keeps its record, so the next start recovers it.
    if settled.is_ok() {
        let _ = store.forget_replacements(&[&pair.staged]);
    }
    match (swapped, settled) {
        (Err(error), Err(stuck)) => Err(io::Error::new(error.kind(), format!("{error}; the original could not be put back ({stuck}) and is kept at {}", pair.backup))),
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
    let (destination, staged, backup) = (Path::new(&pair.destination), Path::new(&pair.staged), Path::new(&pair.backup));
    if !present(destination)? && present(backup)? {
        #[cfg(test)]
        tests::fault(tests::Fault::Restore, destination)?;
        std::fs::rename(backup, destination)?;
    }
    remove_if_present(staged)?;
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
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tag = crate::random_hex(4);
    let sibling = |extension: &str| path.with_file_name(format!(".{name}.drift-{tag}.{extension}")).to_string_lossy().into_owned();
    StagedReplacement { destination: path.to_string_lossy().into_owned(), staged: sibling("tmp"), backup: sibling("bak") }
}

/// Exactly what [`beside`] would make for this destination, with one tag.
fn is_pair(pair: &StagedReplacement) -> bool {
    let destination = Path::new(&pair.destination);
    let Some(name) = destination.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    let Some(tag) = Path::new(&pair.staged).file_name().and_then(|n| n.to_string_lossy().strip_prefix(&format!(".{name}.drift-")).and_then(|rest| rest.strip_suffix(".tmp")).map(str::to_owned)) else {
        return false;
    };
    let expected = |extension: &str| destination.with_file_name(format!(".{name}.drift-{tag}.{extension}")).to_string_lossy().into_owned();
    tag.len() == 8 && tag.chars().all(|c| c.is_ascii_hexdigit()) && pair.staged == expected("tmp") && pair.backup == expected("bak")
}

async fn swap(pair: &StagedReplacement, bytes: &[u8], original: Option<std::fs::Metadata>) -> io::Result<()> {
    let (staged, backup, path) = (Path::new(&pair.staged), Path::new(&pair.backup), Path::new(&pair.destination));
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
    tokio::task::spawn_blocking(move || windows::replace_file(&path, &staged, &backup)).await.map_err(io::Error::other)?
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::store::tests::store;

    #[derive(Clone, Copy, PartialEq)]
    pub(crate) enum Fault {
        /// The staged copy is written, then the replacement fails before the swap.
        AfterStaging,
        /// Moving a backup back over its destination fails.
        Restore,
    }

    static FAULTS: std::sync::Mutex<Vec<(Fault, PathBuf)>> = std::sync::Mutex::new(Vec::new());

    /// Makes the next `fault` at `path` fail, once.
    pub(crate) fn inject(fault: Fault, path: &Path) {
        FAULTS.lock().unwrap().push((fault, path.to_path_buf()));
    }

    pub(super) fn fault(fault: Fault, path: &Path) -> io::Result<()> {
        let mut faults = FAULTS.lock().unwrap();
        match faults.iter().position(|(f, p)| *f == fault && p == path) {
            Some(index) => {
                faults.remove(index);
                Err(io::Error::other("injected failure"))
            }
            None => Ok(()),
        }
    }

    pub(crate) fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("drift-stage-{name}-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".drift-")).collect()
    }

    /// What a crash halfway through a Windows swap leaves: the original at the backup name, the new bytes staged, no destination.
    pub(crate) fn stranded(store: &Store, destination: &Path, original: &str) -> StagedReplacement {
        let pair = beside(destination);
        store.record_replacement(&pair).unwrap();
        std::fs::write(&pair.backup, original).unwrap();
        std::fs::write(&pair.staged, "half of the new").unwrap();
        pair
    }

    #[tokio::test]
    async fn a_replacement_leaves_nothing_of_its_own_behind() {
        let (store, dir) = (store(), sandbox("clean"));
        let file = dir.join("a.txt");
        std::fs::write(&file, "one").unwrap();
        replace(&store, &file, b"two").await.unwrap();
        replace(&store, &dir.join("new/b.txt"), b"fresh").await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert!(leftovers(&dir).is_empty() && store.replacements().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_replacement_that_fails_after_staging_leaves_the_file_and_nothing_else() {
        let (store, dir) = (store(), sandbox("staged-fail"));
        let file = dir.join("a.txt");
        std::fs::write(&file, "one").unwrap();
        inject(Fault::AfterStaging, &file);
        assert!(replace(&store, &file, b"two").await.unwrap_err().to_string().contains("injected"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");
        assert!(leftovers(&dir).is_empty() && store.replacements().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn reopen(dir: &Path) -> Store {
        crate::store::open(&dir.join("data")).unwrap()
    }

    #[test]
    fn on_reopen_a_stranded_backup_is_put_back_not_deleted() {
        let dir = sandbox("stranded");
        let file = dir.join("a.txt");
        let pair = stranded(&reopen(&dir), &file, "the original");
        let store = reopen(&dir);
        assert_eq!(recover_leftovers(&store), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the original");
        assert!(!Path::new(&pair.backup).exists() && !Path::new(&pair.staged).exists());
        assert!(store.replacements().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn opening_the_engine_recovers_before_anything_can_run() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = sandbox("engine");
        let file = dir.join("a.txt");
        stranded(&reopen(&dir), &file, "the original");
        let engine = crate::Engine::open_with(&dir.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the original");
        assert!(engine.store.replacements().unwrap().is_empty());
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_restore_keeps_both_files_and_their_record_until_a_later_start_succeeds() {
        let dir = sandbox("restore-fails");
        let file = dir.join("a.txt");
        let pair = stranded(&reopen(&dir), &file, "the original");
        inject(Fault::Restore, &file);
        let store = reopen(&dir);
        assert_eq!(recover_leftovers(&store), 0);
        assert!(!file.exists() && Path::new(&pair.backup).exists() && Path::new(&pair.staged).exists(), "nothing deleted");
        assert_eq!(store.replacements().unwrap(), std::slice::from_ref(&pair), "still on record");
        drop(store);

        let store = reopen(&dir);
        assert_eq!(recover_leftovers(&store), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the original");
        assert!(leftovers(&dir).iter().all(|name| !name.starts_with(".a.txt")));
        drop(store);
        // Again on a clean slate: nothing to do, nothing touched.
        let store = reopen(&dir);
        assert_eq!(recover_leftovers(&store), 0);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the original");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn on_reopen_a_finished_swap_loses_only_its_siblings_and_unknown_files_are_left() {
        let dir = sandbox("finished");
        let file = dir.join("a.txt");
        let store = reopen(&dir);
        let pair = stranded(&store, &file, "old");
        std::fs::write(&file, "new").unwrap();
        let unrecorded = dir.join(".b.txt.drift-89abcdef.tmp");
        std::fs::write(&unrecorded, "x").unwrap();
        let foreign = StagedReplacement { destination: file.to_string_lossy().into(), staged: dir.join("notes.tmp").to_string_lossy().into(), backup: dir.join("notes.bak").to_string_lossy().into() };
        std::fs::write(&foreign.staged, "keep").unwrap();
        store.record_replacement(&foreign).unwrap();
        drop(store);
        let store = reopen(&dir);
        assert_eq!(recover_leftovers(&store), 2);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new", "the destination is in place, so the backup was the old copy");
        assert!(!Path::new(&pair.backup).exists() && !Path::new(&pair.staged).exists());
        assert!(unrecorded.exists() && Path::new(&foreign.staged).exists(), "never recorded, or not a name the engine gives");
        assert!(store.replacements().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    fn security(file: &Path) -> String {
        let saved = file.with_extension("acl");
        let status = std::process::Command::new("icacls").arg(file).arg("/save").arg(&saved).output().unwrap().status;
        assert!(status.success());
        let raw = std::fs::read(&saved).unwrap();
        let _ = std::fs::remove_file(&saved);
        let units: Vec<u16> = raw.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        String::from_utf16_lossy(&units).lines().nth(1).unwrap_or_default().to_string()
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_replaced_file_keeps_its_explicit_acl() {
        let (store, dir) = (store(), sandbox("acl"));
        let file = dir.join("a.txt");
        std::fs::write(&file, "one").unwrap();
        let granted = std::process::Command::new("icacls").arg(&file).arg("/grant").arg("*S-1-5-32-545:(R)").output().unwrap();
        assert!(granted.status.success());
        let before = security(&file);
        assert!(before.contains(";;;BU)"), "the explicit entry is there to keep: {before}");
        replace(&store, &file, b"two").await.unwrap();
        assert_eq!(security(&file), before);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_file_held_open_without_delete_sharing_is_left_as_it_was() {
        use std::os::windows::fs::OpenOptionsExt;
        let (store, dir) = (store(), sandbox("shared"));
        let file = dir.join("a.txt");
        std::fs::write(&file, "one").unwrap();
        // Read and write sharing, but not delete: what many editors and indexers hold.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(&file).unwrap();
        let error = replace(&store, &file, b"two").await.unwrap_err();
        assert!(error.to_string().contains("another program"), "{error}");
        drop(held);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");
        assert!(leftovers(&dir).is_empty() && store.replacements().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, ERROR_UNABLE_TO_REMOVE_REPLACED};
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub(super) fn replace_file(path: &Path, staged: &Path, backup: &Path) -> io::Result<()> {
        let (replaced, replacement, saved) = (wide(path), wide(staged), wide(backup));
        // SAFETY: three NUL-terminated paths that outlive the call; the reserved pointers are null.
        let done = unsafe { ReplaceFileW(replaced.as_ptr(), replacement.as_ptr(), saved.as_ptr(), 0, std::ptr::null(), std::ptr::null()) };
        if done != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        let code = error.raw_os_error().unwrap_or_default() as u32;
        // The original was moved to the backup name but the new file could not take its place: move it back.
        if code == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
            let _ = std::fs::rename(backup, path);
        }
        if [ERROR_SHARING_VIOLATION, ERROR_ACCESS_DENIED, ERROR_UNABLE_TO_REMOVE_REPLACED].contains(&code) {
            return Err(io::Error::new(error.kind(), format!("{error} (another program may have it open without allowing it to be replaced)")));
        }
        Err(error)
    }
}
