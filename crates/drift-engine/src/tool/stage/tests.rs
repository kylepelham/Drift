use std::path::PathBuf;

use super::*;
use crate::store::tests::store;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Fault {
    /// The staged copy is written, then the replacement fails before the swap.
    AfterStaging,
    /// Moving a backup back over its destination fails.
    Restore,
    /// Removing a backup fails, as when a scanner holds it open.
    RemoveBackup,
}

static FAULTS: std::sync::Mutex<Vec<(Fault, PathBuf)>> = std::sync::Mutex::new(Vec::new());

/// Makes the next `fault` at `path` fail, once.
pub(crate) fn inject(fault: Fault, path: &Path) {
    FAULTS.lock().unwrap().push((fault, path.to_path_buf()));
}

pub(super) fn fault(fault: Fault, path: &Path) -> io::Result<()> {
    let mut faults = FAULTS.lock().unwrap();
    match faults.iter().position(|(queued, at)| *queued == fault && at == path) {
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
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".drift-"))
        .collect()
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
    assert!(
        replace(&store, &file, b"two")
            .await
            .unwrap_err()
            .to_string()
            .contains("injected")
    );
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

#[test]
fn a_backup_left_after_a_finished_swap_never_brings_back_a_file_deleted_since() {
    let dir = sandbox("swapped");
    let file = dir.join("a.txt");
    let store = reopen(&dir);
    let pair = stranded(&store, &file, "old content");
    store.mark_swapped(&pair.staged).unwrap();
    // The file was deleted on purpose after the swap; only the stale backup remains.
    drop(store);
    let store = reopen(&dir);
    assert_eq!(recover_leftovers(&store), 1);
    assert!(!file.exists(), "the deletion stands");
    assert!(!Path::new(&pair.backup).exists() && !Path::new(&pair.staged).exists());
    assert!(store.replacements().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[tokio::test]
async fn a_backup_that_cannot_be_removed_yet_is_recorded_as_old_content() {
    let dir = sandbox("stuck-backup");
    let file = dir.join("a.txt");
    std::fs::write(&file, "old").unwrap();
    let store = reopen(&dir);
    inject(Fault::RemoveBackup, &file);
    replace(&store, &file, b"new").await.unwrap();
    let [left] = store.replacements().unwrap().try_into().unwrap();
    assert!(
        left.swapped && Path::new(&left.backup).exists(),
        "kept on record, marked as past the swap"
    );
    std::fs::remove_file(&file).unwrap();
    drop(store);
    let store = reopen(&dir);
    assert_eq!(recover_leftovers(&store), 1);
    assert!(!file.exists() && leftovers(&dir).is_empty(), "removed, not restored");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn opening_the_engine_recovers_before_anything_can_run() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = sandbox("engine");
    let file = dir.join("a.txt");
    stranded(&reopen(&dir), &file, "the original");
    let engine = crate::Engine::open_with(
        &dir.join("data"),
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
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
    assert!(
        !file.exists() && Path::new(&pair.backup).exists() && Path::new(&pair.staged).exists(),
        "nothing deleted"
    );
    assert_eq!(
        store.replacements().unwrap(),
        std::slice::from_ref(&pair),
        "still on record"
    );
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
    let foreign = StagedReplacement {
        destination: file.to_string_lossy().into(),
        staged: dir.join("notes.tmp").to_string_lossy().into(),
        backup: dir.join("notes.bak").to_string_lossy().into(),
        swapped: false,
    };
    std::fs::write(&foreign.staged, "keep").unwrap();
    store.record_replacement(&foreign).unwrap();
    drop(store);
    let store = reopen(&dir);
    assert_eq!(recover_leftovers(&store), 2);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "new",
        "the destination is in place, so the backup was the old copy"
    );
    assert!(!Path::new(&pair.backup).exists() && !Path::new(&pair.staged).exists());
    assert!(
        unrecorded.exists() && Path::new(&foreign.staged).exists(),
        "never recorded, or not a name the engine gives"
    );
    assert!(store.replacements().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
fn security(file: &Path) -> String {
    let saved = file.with_extension("acl");
    let status = std::process::Command::new("icacls")
        .arg(file)
        .arg("/save")
        .arg(&saved)
        .output()
        .unwrap()
        .status;
    assert!(status.success());
    let raw = std::fs::read(&saved).unwrap();
    let _ = std::fs::remove_file(&saved);
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
        .lines()
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

#[cfg(windows)]
#[tokio::test]
async fn a_replaced_file_keeps_its_explicit_acl() {
    let (store, dir) = (store(), sandbox("acl"));
    let file = dir.join("a.txt");
    std::fs::write(&file, "one").unwrap();
    let granted = std::process::Command::new("icacls")
        .arg(&file)
        .arg("/grant")
        .arg("*S-1-5-32-545:(R)")
        .output()
        .unwrap();
    assert!(granted.status.success());
    let before = security(&file);
    assert!(
        before.contains(";;;BU)"),
        "the explicit entry is there to keep: {before}"
    );
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
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&file)
        .unwrap();
    let error = replace(&store, &file, b"two").await.unwrap_err();
    assert!(error.to_string().contains("another program"), "{error}");
    drop(held);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");
    assert!(leftovers(&dir).is_empty() && store.replacements().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
