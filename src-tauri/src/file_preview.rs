//! Read-only, size-bounded previews within the originating workspace.

mod opened_path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use drift_engine::Engine;
use opened_path::opened_file_path;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_PREVIEW_BYTES: u64 = 40 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
enum PreviewError {
    #[error("File preview maxBytes is too large: maximum is 40 MiB")]
    InvalidLimit,
    #[error("File preview directory must be an absolute workspace path")]
    RelativeDirectory,
    #[error("File preview directory is not a directory")]
    NotDirectory,
    #[error("File preview directory is not in a Drift workspace")]
    UnknownWorkspace,
    #[error("File preview could not list workspaces: {0}")]
    Workspaces(#[source] rusqlite::Error),
    #[error("File preview path is missing")]
    EmptyPath,
    #[error("File preview path is outside the workspace")]
    OutsideWorkspace,
    #[error("File preview path is not a regular file")]
    NotFile,
    #[error("File preview is too large: limit is {0} bytes")]
    TooLarge(u64),
    #[error("File preview opened file is outside the workspace")]
    OpenedOutsideWorkspace,
    #[error("File preview file or directory is missing")]
    Missing,
    #[error("File preview could not be read: {0}")]
    Read(#[source] io::Error),
    #[cfg(any(windows, target_os = "linux"))]
    #[error("File preview cannot validate opened file path: {0}")]
    OpenedPath(#[source] io::Error),
    #[cfg(windows)]
    #[error("File preview cannot validate opened file path: path is too long")]
    PathTooLong,
    #[cfg(target_os = "linux")]
    #[error("File preview cannot validate opened file path: file was deleted")]
    Deleted,
    #[cfg(not(any(windows, target_os = "linux")))]
    #[error("File preview cannot validate opened-file containment on this platform")]
    UnsupportedPlatform,
}

#[derive(Debug, Serialize)]
pub(crate) struct FilePreview {
    content: String,
    size: u64,
}

#[tauri::command]
pub(crate) async fn read_file_preview(path: String, directory: String, max_bytes: u64) -> Result<FilePreview, String> {
    tauri::async_runtime::spawn_blocking(move || read_preview(&path, &directory, max_bytes))
        .await
        .map_err(|error| format!("File preview task failed: {error}"))?
        .map_err(|error| error.to_string())
}

/// A paired device's preview: `directory` must lie in a workspace Drift knows or in one of its worktrees.
pub(crate) async fn read_workspace_preview(
    engine: Arc<Engine>,
    path: String,
    directory: String,
    max_bytes: u64,
) -> Result<FilePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let roots = workspace_roots(&engine).map_err(PreviewError::Workspaces)?;
        read_known_preview(&path, &directory, max_bytes, &roots)
    })
    .await
    .map_err(|error| format!("File preview task failed: {error}"))?
    .map_err(|error| error.to_string())
}

fn workspace_roots(engine: &Engine) -> rusqlite::Result<Vec<PathBuf>> {
    let workspaces = engine.store.workspaces()?;
    let roots = workspaces.into_iter().flat_map(|workspace| {
        let worktrees = store_id(&workspace.id).then(|| engine.worktree_path(&workspace.id, ""));
        [Some(PathBuf::from(workspace.path)), worktrees].into_iter().flatten()
    });

    Ok(roots.collect())
}

/// An id the store makes: 24 lowercase hex digits, so it names one folder under `worktrees` and nothing else.
fn store_id(id: &str) -> bool {
    id.len() == 24 && id.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_known_preview(
    path: &str,
    directory: &str,
    max_bytes: u64,
    roots: &[PathBuf],
) -> Result<FilePreview, PreviewError> {
    check_limit(max_bytes)?;
    read_in_root(path, &known_root(directory, roots)?, max_bytes, |path| File::open(path))
}

/// `directory` resolved once, when it lies in one of `roots`; the read then uses this path, never `directory` again.
fn known_root(directory: &str, roots: &[PathBuf]) -> Result<PathBuf, PreviewError> {
    let directory = Path::new(directory);
    let root = directory
        .is_absolute()
        .then(|| directory.canonicalize().ok())
        .flatten()
        .filter(|directory| {
            roots
                .iter()
                .filter_map(|root| root.canonicalize().ok())
                .any(|root| directory.starts_with(root))
        })
        .ok_or(PreviewError::UnknownWorkspace)?;
    if !root.is_dir() {
        return Err(PreviewError::NotDirectory);
    }

    Ok(root)
}

fn read_preview(path: &str, directory: &str, max_bytes: u64) -> Result<FilePreview, PreviewError> {
    read_preview_with_open(path, directory, max_bytes, |path| File::open(path))
}

fn read_preview_with_open(
    path: &str,
    directory: &str,
    max_bytes: u64,
    open: impl FnOnce(&Path) -> io::Result<File>,
) -> Result<FilePreview, PreviewError> {
    check_limit(max_bytes)?;
    let root = Path::new(directory);
    if !root.is_absolute() {
        return Err(PreviewError::RelativeDirectory);
    }
    let root = root.canonicalize().map_err(io_error)?;
    if !root.is_dir() {
        return Err(PreviewError::NotDirectory);
    }

    read_in_root(path, &root, max_bytes, open)
}

fn check_limit(max_bytes: u64) -> Result<(), PreviewError> {
    if max_bytes > MAX_PREVIEW_BYTES {
        return Err(PreviewError::InvalidLimit);
    }

    Ok(())
}

/// Reads `path` inside `root`, which the caller has already made canonical, within a checked `max_bytes`.
fn read_in_root(
    path: &str,
    root: &Path,
    max_bytes: u64,
    open: impl FnOnce(&Path) -> io::Result<File>,
) -> Result<FilePreview, PreviewError> {
    if path.is_empty() {
        return Err(PreviewError::EmptyPath);
    }

    let path = root.join(path).canonicalize().map_err(io_error)?;
    if !path.starts_with(root) {
        return Err(PreviewError::OutsideWorkspace);
    }

    let metadata = fs::metadata(&path).map_err(io_error)?;
    if !metadata.is_file() {
        return Err(PreviewError::NotFile);
    }
    if metadata.len() > max_bytes {
        return Err(PreviewError::TooLarge(max_bytes));
    }

    let file = open(&path).map_err(io_error)?;
    // Validate the open handle because its checked pathname may have been replaced before open.
    let opened_path = opened_file_path(&file)?;
    if !opened_path.is_absolute() || !opened_path.starts_with(root) {
        return Err(PreviewError::OpenedOutsideWorkspace);
    }

    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err(PreviewError::NotFile);
    }
    if metadata.len() > max_bytes {
        return Err(PreviewError::TooLarge(max_bytes));
    }

    read_bounded(file, max_bytes)
}

fn read_bounded(reader: impl Read, max_bytes: u64) -> Result<FilePreview, PreviewError> {
    // The extra byte detects growth after metadata without an unbounded allocation/read.
    let mut bytes = Vec::new();
    reader.take(max_bytes + 1).read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() as u64 > max_bytes {
        return Err(PreviewError::TooLarge(max_bytes));
    }

    Ok(FilePreview {
        size: bytes.len() as u64,
        content: STANDARD.encode(bytes),
    })
}

fn io_error(error: io::Error) -> PreviewError {
    if error.kind() == io::ErrorKind::NotFound {
        PreviewError::Missing
    } else {
        PreviewError::Read(error)
    }
}

#[cfg(all(test, not(any(windows, target_os = "linux"))))]
#[test]
fn file_preview_handle_validation_fails_closed_on_unsupported_platforms() {
    let file = File::open(std::env::current_exe().unwrap()).unwrap();
    assert!(matches!(
        opened_file_path(&file),
        Err(PreviewError::UnsupportedPlatform)
    ));
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).unwrap();
            let root = drift_engine::platform::temp::test_run().join(format!("drift-file-preview-{random:x?}"));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("workspace")).unwrap();
            Self(root)
        }

        fn workspace(&self) -> PathBuf {
            self.0.join("workspace")
        }

        fn read(&self, path: &str, max_bytes: u64) -> Result<FilePreview, PreviewError> {
            read_preview(path, self.workspace().to_str().unwrap(), max_bytes)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_preview_preserves_exact_whitespace_binary_and_empty_bytes() {
        let fixture = Fixture::new();
        for bytes in [b" \t\r\nhello\n\n  ".as_slice(), b"\0\xff\x80\r\n", b""] {
            let path = fixture.workspace().join("preview.bin");
            fs::write(&path, bytes).unwrap();
            for input in ["preview.bin", path.to_str().unwrap()] {
                let preview = fixture.read(input, bytes.len() as u64).unwrap();
                let json = serde_json::to_value(&preview).unwrap();
                assert_eq!(
                    json,
                    serde_json::json!({
                        "content": STANDARD.encode(bytes), "size": bytes.len() as u64,
                    })
                );
                assert_eq!(STANDARD.decode(preview.content).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn file_preview_rejects_missing_files_and_non_files() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.read("missing.txt", 100).unwrap_err().to_string(),
            "File preview file or directory is missing"
        );
        assert_eq!(
            fixture.read("", 100).unwrap_err().to_string(),
            "File preview path is missing"
        );
        assert_eq!(
            fixture.read(".", 100).unwrap_err().to_string(),
            "File preview path is not a regular file"
        );
    }

    #[test]
    fn file_preview_requires_an_absolute_existing_workspace_directory() {
        let fixture = Fixture::new();
        for directory in ["", ".", "workspace", "../workspace"] {
            assert!(matches!(
                read_preview("file", directory, 1),
                Err(PreviewError::RelativeDirectory)
            ));
        }
        let missing = fixture.0.join("missing");
        assert!(
            read_preview("file", missing.to_str().unwrap(), 1)
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        let file = fixture.0.join("file");
        fs::write(&file, b"").unwrap();
        assert!(
            read_preview("file", file.to_str().unwrap(), 1)
                .unwrap_err()
                .to_string()
                .contains("not a directory")
        );
    }

    #[test]
    fn file_preview_rejects_oversize_metadata_and_unsafe_limits() {
        let fixture = Fixture::new();
        let path = fixture.workspace().join("large.bin");
        File::create(&path).unwrap().set_len(MAX_PREVIEW_BYTES + 1).unwrap();
        assert!(
            fixture
                .read("large.bin", MAX_PREVIEW_BYTES)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
        fs::write(&path, b"12").unwrap();
        for limit in [0, 1, MAX_PREVIEW_BYTES + 1, u64::MAX] {
            assert!(
                fixture
                    .read("large.bin", limit)
                    .unwrap_err()
                    .to_string()
                    .contains("too large")
            );
        }
        assert_eq!(fixture.read("large.bin", MAX_PREVIEW_BYTES).unwrap().size, 2);
    }

    #[test]
    fn file_preview_rejects_growth_after_metadata_with_a_bounded_read() {
        let fixture = Fixture::new();
        let path = fixture.workspace().join("growing.bin");
        fs::write(&path, b"1").unwrap();
        let file = File::open(&path).unwrap();
        let limit = file.metadata().unwrap().len();
        File::options().write(true).open(&path).unwrap().set_len(100).unwrap();
        let mut reader = file;
        assert!(matches!(
            read_bounded(&mut reader, limit),
            Err(PreviewError::TooLarge(_))
        ));
        assert_eq!(io::Seek::stream_position(&mut reader).unwrap(), limit + 1);
        assert!(matches!(read_bounded(io::repeat(0), 1), Err(PreviewError::TooLarge(1))));
    }

    #[test]
    fn file_preview_rejects_traversal_absolute_escapes_and_sibling_prefixes() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("secret"), b"secret").unwrap();
        let sibling = fixture.0.join("workspace-other");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("secret"), b"secret").unwrap();
        for path in [
            PathBuf::from("../secret"),
            fixture.0.join("secret"),
            sibling.join("secret"),
        ] {
            assert!(
                fixture
                    .read(path.to_str().unwrap(), 100)
                    .unwrap_err()
                    .to_string()
                    .contains("outside the workspace")
            );
        }
    }

    #[test]
    fn file_preview_rejects_swapped_file_ancestor_and_workspace_before_reading() {
        for swap in ["file", "ancestor", "workspace"] {
            let fixture = Fixture::new();
            fs::create_dir(fixture.workspace().join("nested")).unwrap();
            fs::write(fixture.workspace().join("nested/file"), b"public").unwrap();
            let outside = fixture.0.join("outside");
            fs::create_dir_all(outside.join("nested")).unwrap();
            fs::write(outside.join("file"), b"secret").unwrap();
            fs::write(outside.join("nested/file"), b"secret").unwrap();
            let (replaced, target) = match swap {
                "file" => (fixture.workspace().join("nested/file"), outside.join("file")),
                "ancestor" => (fixture.workspace().join("nested"), outside.clone()),
                _ => (fixture.workspace(), outside.clone()),
            };
            let link = fixture.0.join("replacement-link");
            #[cfg(target_os = "linux")]
            let result = std::os::unix::fs::symlink(&target, &link);
            #[cfg(windows)]
            let result = if swap == "file" {
                std::os::windows::fs::symlink_file(&target, &link)
            } else {
                std::os::windows::fs::symlink_dir(&target, &link)
            };
            #[cfg(windows)]
            if result
                .as_ref()
                .err()
                .is_some_and(|error| error.raw_os_error() == Some(1314))
            {
                eprintln!("swap fixture skipped: Windows requires Developer Mode or symlink privilege");
                return;
            }
            result.unwrap();

            let saved = fixture.0.join("saved");
            let mut witness = None;
            let error = read_preview_with_open("nested/file", fixture.workspace().to_str().unwrap(), 6, |path| {
                fs::rename(&replaced, &saved)?;
                fs::rename(&link, &replaced)?;
                let opened = File::open(path);
                // Restore the pathname so only the open handle can identify the outside file.
                fs::rename(&replaced, &link)?;
                fs::rename(&saved, &replaced)?;
                let file = opened?;
                assert!(
                    path.canonicalize()
                        .unwrap()
                        .starts_with(fixture.workspace().canonicalize().unwrap())
                );
                assert!(
                    opened_file_path(&file)
                        .unwrap()
                        .starts_with(outside.canonicalize().unwrap())
                );
                witness = Some(file.try_clone()?);
                Ok(file)
            })
            .unwrap_err();
            assert!(matches!(error, PreviewError::OpenedOutsideWorkspace), "{swap}: {error}");
            // try_clone shares the file cursor, so a zero position proves no bytes were read.
            assert_eq!(io::Seek::stream_position(&mut witness.unwrap()).unwrap(), 0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn file_preview_handle_paths_match_the_canonical_extended_namespace() {
        let fixture = Fixture::new();
        let root = fixture.workspace().canonicalize().unwrap();
        assert!(root.to_str().unwrap().starts_with(r"\\?\"));
        let directory = root.join("a".repeat(100)).join("b".repeat(100));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("file");
        fs::write(&path, b"long path").unwrap();
        assert!(path.as_os_str().len() > 260);
        assert_eq!(
            opened_file_path(&File::open(&path).unwrap()).unwrap(),
            path.canonicalize().unwrap()
        );
        for directory in [fixture.workspace(), root] {
            let preview = read_preview(path.to_str().unwrap(), directory.to_str().unwrap(), 9).unwrap();
            assert_eq!(STANDARD.decode(preview.content).unwrap(), b"long path");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_preview_rejects_deleted_opened_files() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace().join("file"), b"public").unwrap();
        let error = read_preview_with_open("file", fixture.workspace().to_str().unwrap(), 6, |path| {
            let file = File::open(path)?;
            fs::remove_file(path)?;
            Ok(file)
        })
        .unwrap_err();
        assert!(matches!(error, PreviewError::Deleted));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn file_preview_rejects_symlink_escapes_and_canonicalizes_workspace() {
        let fixture = Fixture::new();
        let target = fixture.0.join("secret");
        fs::write(&target, b"secret").unwrap();
        let link = fixture.workspace().join("escape");
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(windows)]
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.raw_os_error() == Some(1314))
        {
            eprintln!("symlink fixture skipped: Windows requires Developer Mode or symlink privilege");
            return;
        }
        result.unwrap();
        assert!(
            fixture
                .read("escape", 100)
                .unwrap_err()
                .to_string()
                .contains("outside the workspace")
        );

        let directory_link = fixture.workspace().join("outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fixture.0, &directory_link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&fixture.0, &directory_link).unwrap();
        assert!(
            fixture
                .read("outside/secret", 100)
                .unwrap_err()
                .to_string()
                .contains("outside the workspace")
        );

        fs::write(fixture.workspace().join("inside"), b"inside").unwrap();
        let root_link = fixture.0.join("root-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(fixture.workspace(), &root_link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(fixture.workspace(), &root_link).unwrap();
        assert_eq!(read_preview("inside", root_link.to_str().unwrap(), 6).unwrap().size, 6);
    }

    #[test]
    fn file_preview_for_a_paired_device_stays_inside_known_workspaces() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace().join("inside"), b"inside").unwrap();
        fs::create_dir(fixture.workspace().join("nested")).unwrap();
        fs::write(fixture.workspace().join("nested/deeper"), b"deeper").unwrap();
        fs::write(fixture.0.join("secret"), b"secret").unwrap();
        let roots = [fixture.workspace(), fixture.0.join("missing-worktrees")];
        let read = |path: &str, directory: &Path| read_known_preview(path, directory.to_str().unwrap(), 100, &roots);

        assert_eq!(read("inside", &fixture.workspace()).unwrap().size, 6);
        assert_eq!(read("deeper", &fixture.workspace().join("nested")).unwrap().size, 6);
        for directory in [
            fixture.0.clone(),
            fixture.0.join("workspace-other"),
            PathBuf::from("workspace"),
        ] {
            assert!(matches!(
                read("secret", &directory),
                Err(PreviewError::UnknownWorkspace)
            ));
        }
        assert!(matches!(
            read("secret", &fixture.workspace().join("..")),
            Err(PreviewError::UnknownWorkspace)
        ));
        assert!(matches!(
            read_known_preview("inside", fixture.workspace().to_str().unwrap(), 100, &[]),
            Err(PreviewError::UnknownWorkspace)
        ));
    }

    #[test]
    fn file_preview_takes_worktrees_only_under_ids_the_store_makes() {
        assert!(store_id("0123456789abcdef01234567"));
        for id in [
            "",
            "..",
            r"..\..",
            "../..",
            r"C:\",
            "/",
            "0123456789ABCDEF01234567",
            "0123456789abcdef0123456",
            "0123456789abcdef012345678",
            r"0123456789abcdef\..\4567",
        ] {
            assert!(!store_id(id), "{id}");
        }
    }

    #[test]
    fn file_preview_reads_in_the_root_it_checked_even_if_the_directory_is_swapped() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace().join("file"), b"inside").unwrap();
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), b"secret").unwrap();
        let link = fixture.0.join("link");
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(fixture.workspace(), &link);
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_dir(fixture.workspace(), &link);
        #[cfg(windows)]
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.raw_os_error() == Some(1314))
        {
            eprintln!("swap fixture skipped: Windows requires Developer Mode or symlink privilege");
            return;
        }
        result.unwrap();

        let root = known_root(link.to_str().unwrap(), &[fixture.workspace()]).unwrap();
        assert_eq!(root, fixture.workspace().canonicalize().unwrap());
        #[cfg(unix)]
        {
            fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(&outside, &link).unwrap();
        }
        #[cfg(windows)]
        {
            fs::remove_dir(&link).unwrap();
            std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
        }
        let preview = read_in_root("file", &root, 6, |path| File::open(path)).unwrap();
        assert_eq!(STANDARD.decode(preview.content).unwrap(), b"inside");
    }

    #[tokio::test]
    async fn file_preview_async_command_returns_serializable_bytes() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace().join("file"), b"\0\xff \n").unwrap();
        let preview = read_file_preview("file".into(), fixture.workspace().to_str().unwrap().into(), 4)
            .await
            .unwrap();
        assert_eq!(preview.size, 4);
        assert_eq!(STANDARD.decode(preview.content).unwrap(), b"\0\xff \n");
    }
}
