//! Writers of one file take turns, across sessions and workers: a call holds its files from the
//! snapshot before it through the record after it, so two edits never start from the same bytes
//! and a change is never attributed to the wrong call. Shell commands name no files and take none.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Mutex as Turn, OwnedMutexGuard};

static LOCKS: Mutex<Option<HashMap<PathBuf, Weak<Turn<()>>>>> = Mutex::new(None);

/// Held files; dropping it lets the next writer of each go.
pub struct Held {
    _turns: Vec<OwnedMutexGuard<()>>,
}

/// Waits for every one of `paths`, taken in one order so two callers never hold each other up.
pub async fn files(paths: &[PathBuf]) -> Held {
    let mut paths: Vec<PathBuf> = paths.iter().map(|path| super::canonical(path)).collect();
    paths.sort();
    paths.dedup();
    let turns: Vec<Arc<Turn<()>>> = {
        let mut locks = LOCKS.lock().unwrap();
        let locks = locks.get_or_insert_with(HashMap::new);
        locks.retain(|_, turn| turn.strong_count() > 0);
        paths.into_iter().map(|path| turn_for(locks, path)).collect()
    };
    let mut held = Vec::with_capacity(turns.len());
    for turn in turns {
        held.push(turn.lock_owned().await);
    }
    Held { _turns: held }
}

fn turn_for(locks: &mut HashMap<PathBuf, Weak<Turn<()>>>, path: PathBuf) -> Arc<Turn<()>> {
    if let Some(turn) = locks.get(&path).and_then(Weak::upgrade) {
        return turn;
    }
    let turn = Arc::new(Turn::new(()));
    locks.insert(path, Arc::downgrade(&turn));
    turn
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_second_writer_of_a_file_waits_for_the_first() {
        let path = std::env::temp_dir().join(format!("drift-lock-{}.txt", crate::random_hex(4)));
        let first = files(std::slice::from_ref(&path)).await;
        let waiting = tokio::spawn({
            let path = path.clone();
            async move { files(&[path]).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiting.is_finished(), "the second writer waits");
        let other = tokio::time::timeout(Duration::from_millis(200), files(&[path.with_extension("other")])).await;
        assert!(other.is_ok(), "another file does not wait");
        drop(first);
        assert!(tokio::time::timeout(Duration::from_secs(2), waiting).await.is_ok(), "and goes once the first is done");
    }
}
