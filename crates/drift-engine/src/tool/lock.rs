//! Atomic writer reservations by canonical path, including overlapping directory roots.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tokio::sync::Notify;

#[derive(Clone)]
enum Scope {
    Files(Vec<PathBuf>),
    Tree(PathBuf),
}

impl Scope {
    fn conflicts(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Files(mine), Self::Files(theirs)) => mine.iter().any(|path| theirs.binary_search(path).is_ok()),
            (Self::Tree(mine), Self::Tree(theirs)) => mine.starts_with(theirs) || theirs.starts_with(mine),
            (Self::Tree(root), Self::Files(paths)) | (Self::Files(paths), Self::Tree(root)) => {
                paths.iter().any(|path| path.starts_with(root))
            }
        }
    }
}

#[derive(Default)]
struct Reservations {
    next: u64,
    active: BTreeMap<u64, Scope>,
    waiting: VecDeque<(u64, Scope)>,
}

#[derive(Default)]
struct Writers {
    reservations: Mutex<Reservations>,
    changed: Notify,
}

fn writers() -> &'static Writers {
    static WRITERS: OnceLock<Writers> = OnceLock::new();
    WRITERS.get_or_init(Writers::default)
}

/// Dropping a reservation releases its complete path set or directory tree.
pub(crate) struct Held(u64);

impl Drop for Held {
    fn drop(&mut self) {
        writers().reservations.lock().unwrap().active.remove(&self.0);
        writers().changed.notify_waiters();
    }
}

struct Pending(u64);

impl Drop for Pending {
    fn drop(&mut self) {
        writers()
            .reservations
            .lock()
            .unwrap()
            .waiting
            .retain(|(id, _)| *id != self.0);
        writers().changed.notify_waiters();
    }
}

/// Reserves all physical paths at once; dropping a waiting future cancels without retaining any paths.
pub(crate) async fn files(paths: &[PathBuf]) -> Held {
    let mut paths: Vec<PathBuf> = paths.iter().map(|path| path_key(path)).collect();
    paths.sort();
    paths.dedup();

    acquire(Scope::Files(paths)).await
}

/// Excludes all writers beneath this root, irrespective of the workspace each writer belongs to.
pub(crate) async fn workspace(root: &Path) -> Held {
    acquire(Scope::Tree(path_key(root))).await
}

/// Case-folded reservation identity on Windows; callers retain the real path for file operations.
pub(crate) fn path_key(path: &Path) -> PathBuf {
    let path = super::canonical(path);
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().to_lowercase());
    path
}

async fn acquire(scope: Scope) -> Held {
    let pending = {
        let mut state = writers().reservations.lock().unwrap();
        let id = state.next;
        state.next += 1;
        state.waiting.push_back((id, scope.clone()));
        Pending(id)
    };

    loop {
        let changed = writers().changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if grant(pending.0, &scope) {
            return Held(pending.0);
        }
        changed.await;
    }
}

fn grant(id: u64, scope: &Scope) -> bool {
    let mut state = writers().reservations.lock().unwrap();
    let active = state.active.values().any(|other| scope.conflicts(other));
    let earlier = state
        .waiting
        .iter()
        .take_while(|(waiting, _)| *waiting != id)
        .any(|(_, other)| scope.conflicts(other));
    if active || earlier {
        return false;
    }

    state.waiting.retain(|(waiting, _)| *waiting != id);
    state.active.insert(id, scope.clone());
    true
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("drift-lock-{}", crate::random_hex(4)))
    }

    #[tokio::test]
    async fn file_reservations_are_atomic_deduplicated_and_cancellable() {
        let root = root();
        let (a, b) = (root.join("a.txt"), root.join("b.txt"));
        let held = files(std::slice::from_ref(&b)).await;
        let waiting = tokio::spawn({
            let (a, b) = (a.clone(), b.clone());
            async move { files(&[a.clone(), b, a]).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiting.is_finished());
        let other = tokio::time::timeout(Duration::from_millis(100), files(std::slice::from_ref(&a))).await;
        assert!(
            other.is_err(),
            "later conflicting writers cannot bypass an earlier reservation"
        );
        waiting.abort();
        let _ = waiting.await;
        let independent = tokio::time::timeout(Duration::from_secs(1), files(std::slice::from_ref(&a)))
            .await
            .unwrap();
        drop(independent);
        drop(held);
        let both = tokio::time::timeout(Duration::from_secs(1), files(&[a.clone(), b, a]))
            .await
            .unwrap();
        drop(both);
    }

    #[tokio::test]
    async fn overlapping_trees_and_outside_writes_conflict_by_physical_path() {
        let root = root();
        let nested = root.join("sub");
        let held = workspace(&root).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), workspace(&nested))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), files(&[nested.join("a.rs")]))
                .await
                .is_err()
        );
        let elsewhere = root.with_extension("other");
        let independent = tokio::time::timeout(Duration::from_secs(1), files(&[elsewhere.join("a.rs")]))
            .await
            .unwrap();
        drop(independent);
        drop(held);
        let outside = files(&[nested.join("a.rs")]).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), workspace(&root))
                .await
                .is_err()
        );
        drop(outside);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), workspace(&root))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn reversed_multi_file_requests_cannot_deadlock() {
        let root = root();
        let (a, b) = (root.join("a"), root.join("b"));
        let first = files(&[b.clone(), a.clone()]).await;
        let second = tokio::spawn(async move { files(&[a, b]).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!second.is_finished());
        drop(first);
        assert!(tokio::time::timeout(Duration::from_secs(1), second).await.is_ok());
    }
}
