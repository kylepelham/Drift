//! The writer and the readers beside it. WAL lets a reader see every committed write while the writer works,
//! so reads never queue behind writes; waiting for a connection, and any long read, happens off the async workers.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

use rusqlite::{Connection, OpenFlags};

/// Read connections beside the writer: enough for the UI, turns and workers to read at once.
const READERS: usize = 4;

/// Read-only connections to the same file; none for an in-memory database, which only the writer can see.
pub(super) struct Readers {
    slots: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl Readers {
    #[cfg(test)]
    pub(super) fn none() -> Self {
        Self {
            slots: Vec::new(),
            next: AtomicUsize::new(0),
        }
    }

    pub(super) fn open(file: &Path) -> rusqlite::Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let slots = (0..READERS)
            .map(|_| {
                let conn = Connection::open_with_flags(file, flags)?;
                conn.busy_timeout(super::BUSY_TIMEOUT)?;
                conn.pragma_update(None, "mmap_size", super::MMAP_SIZE_BYTES)?;
                Ok(Mutex::new(conn))
            })
            .collect::<rusqlite::Result<_>>()?;

        Ok(Self {
            slots,
            next: AtomicUsize::new(0),
        })
    }

    /// A free reader, else the next in turn once it is free; `None` when there are no readers.
    pub(super) fn take(&self) -> Option<MutexGuard<'_, Connection>> {
        if self.slots.is_empty() {
            return None;
        }
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        let count = self.slots.len();
        let free = (0..count).find_map(|offset| match self.slots[(start + offset) % count].try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        });

        Some(free.unwrap_or_else(|| acquire(&self.slots[start % count])))
    }
}

/// The guard, waiting off the async workers when another holder has it.
pub(super) fn acquire(mutex: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    match mutex.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => blocking(|| mutex.lock().unwrap_or_else(PoisonError::into_inner)),
    }
}

/// Runs `work` as blocking work: on a multi-threaded runtime's worker the worker's other tasks move to another
/// thread first; anywhere else (a plain thread, a current-thread runtime) it simply runs.
pub fn blocking<T>(work: impl FnOnce() -> T) -> T {
    let multi_threaded = tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    if multi_threaded {
        tokio::task::block_in_place(work)
    } else {
        work()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reader_sees_what_the_writer_committed_while_the_writer_is_held() {
        let dir = crate::platform::temp::test_run().join(format!("store-readers-{}", crate::random_hex(4)));
        let store = crate::store::open(&dir).unwrap();
        store.add_workspace("C:/repo", "repo", "").unwrap();

        let held = store.lock();
        let listed = store.workspaces().unwrap();
        drop(held);

        assert_eq!(listed.len(), 1, "read on a reader without waiting for the writer");
    }

    #[test]
    fn blocking_work_runs_off_any_runtime_and_on_a_current_thread_one() {
        assert_eq!(blocking(|| 1), 1);
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert_eq!(runtime.block_on(async { blocking(|| 2) }), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_held_connection_does_not_stall_the_workers_other_tasks() {
        let mutex = std::sync::Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let (taken, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
        let holder = {
            let mutex = mutex.clone();
            std::thread::spawn(move || {
                let _held = mutex.lock().unwrap();
                taken.0.send(()).unwrap();
                release.1.recv().unwrap();
            })
        };
        taken.1.recv().unwrap();
        let waiter = {
            let mutex = mutex.clone();
            tokio::spawn(async move { drop(acquire(&mutex)) })
        };

        let other = tokio::time::timeout(std::time::Duration::from_secs(2), tokio::spawn(async { 7 })).await;
        release.0.send(()).unwrap();

        assert_eq!(other.unwrap().unwrap(), 7, "the lone worker kept running other tasks");
        waiter.await.unwrap();
        holder.join().unwrap();
    }
}
