//! Writers of one file take turns across sessions; see "Writers of one file take turns" in docs/engine-rewrite.md.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Mutex as Turn, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

type Turns<T> = Mutex<Option<HashMap<PathBuf, Weak<T>>>>;

static FILES: Turns<Turn<()>> = Mutex::new(None);
static WORKSPACES: Turns<RwLock<()>> = Mutex::new(None);

/// Held turns; dropping it lets the next writer go.
pub struct Held {
    _files: Vec<OwnedMutexGuard<()>>,
    _shared: Option<OwnedRwLockReadGuard<()>>,
    _whole: Option<OwnedRwLockWriteGuard<()>>,
}

/// Waits for every one of `paths` in `workspace`, taken in one order so two callers never hold each other up.
pub async fn files(workspace: &Path, paths: &[PathBuf]) -> Held {
    let shared = entry(&WORKSPACES, super::canonical(workspace), || RwLock::new(())).read_owned().await;
    let mut paths: Vec<PathBuf> = paths.iter().map(|path| super::canonical(path)).collect();
    paths.sort();
    paths.dedup();
    let turns: Vec<Arc<Turn<()>>> = paths.into_iter().map(|path| entry(&FILES, path, || Turn::new(()))).collect();
    let mut held = Vec::with_capacity(turns.len());
    for turn in turns {
        held.push(turn.lock_owned().await);
    }
    Held { _files: held, _shared: Some(shared), _whole: None }
}

/// Waits until no file of `workspace` is held, and holds them all: for a writer that may touch any of them.
pub async fn workspace(workspace: &Path) -> Held {
    let whole = entry(&WORKSPACES, super::canonical(workspace), || RwLock::new(())).write_owned().await;
    Held { _files: Vec::new(), _shared: None, _whole: Some(whole) }
}

fn entry<T>(turns: &Turns<T>, key: PathBuf, make: impl FnOnce() -> T) -> Arc<T> {
    let mut turns = turns.lock().unwrap();
    let turns = turns.get_or_insert_with(HashMap::new);
    turns.retain(|_, turn| turn.strong_count() > 0);
    if let Some(turn) = turns.get(&key).and_then(Weak::upgrade) {
        return turn;
    }
    let turn = Arc::new(make());
    turns.insert(key, Arc::downgrade(&turn));
    turn
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_second_writer_of_a_file_waits_for_the_first() {
        let root = std::env::temp_dir().join(format!("drift-lock-{}", crate::random_hex(4)));
        let path = root.join("a.txt");
        let first = files(&root, std::slice::from_ref(&path)).await;
        let waiting = tokio::spawn({
            let (root, path) = (root.clone(), path.clone());
            async move { files(&root, &[path]).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiting.is_finished(), "the second writer waits");
        let other = tokio::time::timeout(Duration::from_millis(200), files(&root, &[root.join("b.txt")])).await;
        assert!(other.is_ok(), "another file does not wait");
        drop(other);
        drop(first);
        assert!(tokio::time::timeout(Duration::from_secs(2), waiting).await.is_ok(), "and goes once the first is done");
    }

    #[tokio::test]
    async fn a_whole_workspace_writer_waits_for_every_file_writer_and_they_for_it() {
        let root = std::env::temp_dir().join(format!("drift-lock-ws-{}", crate::random_hex(4)));
        let one = files(&root, &[root.join("a.txt")]).await;
        let whole = tokio::spawn({
            let root = root.clone();
            async move { workspace(&root).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!whole.is_finished(), "a file is being written");
        drop(one);
        let whole = tokio::time::timeout(Duration::from_secs(2), whole).await.unwrap().unwrap();
        let blocked = tokio::time::timeout(Duration::from_millis(100), files(&root, &[root.join("b.txt")])).await;
        assert!(blocked.is_err(), "any file waits while the whole workspace is held");
        let elsewhere = std::env::temp_dir().join(format!("drift-lock-other-{}", crate::random_hex(4)));
        assert!(tokio::time::timeout(Duration::from_millis(200), files(&elsewhere, &[elsewhere.join("b.txt")])).await.is_ok(), "another workspace does not");
        drop(whole);
    }
}
