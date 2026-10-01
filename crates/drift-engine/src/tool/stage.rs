//! Whole-file replacement: written beside the file, then swapped in, so a failed write never cuts it short.

use std::io;
use std::path::{Path, PathBuf};

use crate::store::Store;

/// Writes `bytes` to `path` through an engine-owned sibling recorded in the store before it exists.
pub(crate) async fn replace(store: &Store, path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let original = tokio::fs::metadata(path).await.ok();
    if original.as_ref().is_some_and(|meta| meta.permissions().readonly()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the file is read-only"));
    }
    let (staged, backup) = siblings(path);
    let (staged_name, backup_name) = (staged.to_string_lossy().into_owned(), backup.to_string_lossy().into_owned());
    store.record_staged(&[&staged_name, &backup_name]).map_err(io::Error::other)?;
    let swapped = swap(&staged, &backup, path, bytes, original).await;
    if tokio::fs::remove_file(&staged).await.is_ok() || !staged.exists() {
        let _ = store.forget_staged(&[&staged_name]);
    }
    // A backup is disposable only while the file is in place; otherwise it is the only copy, no longer the engine's.
    let orphaned = !path.exists() && backup.exists();
    if !orphaned {
        let _ = tokio::fs::remove_file(&backup).await;
    }
    let _ = store.forget_staged(&[&backup_name]);
    match swapped {
        Err(error) if orphaned => Err(io::Error::new(error.kind(), format!("{error}; the original was left at {}", backup.display()))),
        result => result,
    }
}

/// Removes staged copies a crash left behind: only paths the store recorded and only names the engine gives them.
pub(crate) fn clean_leftovers(store: &Store) -> usize {
    let mut removed = 0;
    for name in store.staged_files().unwrap_or_default() {
        let path = PathBuf::from(&name);
        if is_staged_name(&path) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
        let _ = store.forget_staged(&[&name]);
    }
    removed
}

fn siblings(path: &Path) -> (PathBuf, PathBuf) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tag = crate::random_hex(4);
    (path.with_file_name(format!(".{name}.drift-{tag}.tmp")), path.with_file_name(format!(".{name}.drift-{tag}.bak")))
}

fn is_staged_name(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(stem) = name.strip_suffix(".tmp").or_else(|| name.strip_suffix(".bak")) else { return false };
    let Some((head, tag)) = stem.rsplit_once(".drift-") else { return false };
    head.starts_with('.') && tag.len() == 8 && tag.chars().all(|c| c.is_ascii_hexdigit())
}

async fn swap(staged: &Path, backup: &Path, path: &Path, bytes: &[u8], original: Option<std::fs::Metadata>) -> io::Result<()> {
    tokio::fs::write(staged, bytes).await?;
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
mod tests {
    use super::*;
    use crate::store::tests::store;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("drift-stage-{name}-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".drift-")).collect()
    }

    #[tokio::test]
    async fn a_replacement_leaves_nothing_of_its_own_behind() {
        let (store, dir) = (store(), sandbox("clean"));
        let file = dir.join("a.txt");
        std::fs::write(&file, "one").unwrap();
        replace(&store, &file, b"two").await.unwrap();
        replace(&store, &dir.join("new/b.txt"), b"fresh").await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert!(leftovers(&dir).is_empty() && store.staged_files().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn after_a_crash_only_recorded_engine_named_files_are_removed() {
        let (store, dir) = (store(), sandbox("crash"));
        let ours = dir.join(".a.txt.drift-0123abcd.tmp");
        let unrecorded = dir.join(".b.txt.drift-89abcdef.tmp");
        let misnamed = dir.join("notes.tmp");
        for file in [&ours, &unrecorded, &misnamed] {
            std::fs::write(file, "x").unwrap();
        }
        store.record_staged(&[&ours.to_string_lossy(), &misnamed.to_string_lossy()]).unwrap();
        assert_eq!(clean_leftovers(&store), 1);
        assert!(!ours.exists(), "recorded and named as ours");
        assert!(unrecorded.exists() && misnamed.exists(), "never recorded, or not a name the engine gives");
        assert!(store.staged_files().unwrap().is_empty());
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
        assert!(leftovers(&dir).is_empty() && store.staged_files().unwrap().is_empty());
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
