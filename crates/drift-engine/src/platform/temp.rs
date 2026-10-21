//! Drift's one folder in the system's temporary directory: everything Drift puts there lives in a subfolder of it.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// A test run's folder older than this belongs to a run that has ended.
const ENDED_RUN: Duration = Duration::from_secs(60 * 60);

/// `Drift` in the system's temporary directory.
pub fn root() -> PathBuf {
    std::env::temp_dir().join("Drift")
}

/// A subfolder of [`root`], made if it is missing.
pub fn folder(name: &str) -> PathBuf {
    let dir = root().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// This process's own folder under `tests`, for everything its tests write. The first call removes the
/// folders of runs that have ended, in the background, since a test cannot delete files an engine still holds open.
pub fn test_run() -> PathBuf {
    static RUN: OnceLock<PathBuf> = OnceLock::new();
    RUN.get_or_init(|| {
        let tests = folder("tests");
        let run = tests.join(format!("{}-{}", std::process::id(), crate::random_hex(4)));
        let _ = std::fs::create_dir_all(&run);
        std::thread::spawn(move || sweep_ended_runs(&tests));
        run
    })
    .clone()
}

fn sweep_ended_runs(tests: &std::path::Path) {
    for entry in std::fs::read_dir(tests).into_iter().flatten().flatten() {
        let ended = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > ENDED_RUN));
        if ended {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
