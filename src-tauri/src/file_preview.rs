//! Read-only, size-bounded previews within the originating workspace.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_PREVIEW_BYTES: u64 = 40 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct FilePreview {
    content: String,
    size: u64,
}

#[tauri::command]
pub(crate) async fn read_file_preview(
    path: String,
    directory: String,
    max_bytes: u64,
) -> Result<FilePreview, String> {
    tauri::async_runtime::spawn_blocking(move || read_preview(&path, &directory, max_bytes))
        .await
        .map_err(|error| format!("File preview task failed: {error}"))?
}

fn read_preview(path: &str, directory: &str, max_bytes: u64) -> Result<FilePreview, String> {
    read_preview_with_open(path, directory, max_bytes, |path| File::open(path))
}

fn read_preview_with_open(
    path: &str,
    directory: &str,
    max_bytes: u64,
    open: impl FnOnce(&Path) -> io::Result<File>,
) -> Result<FilePreview, String> {
    if max_bytes > MAX_PREVIEW_BYTES {
        return Err("File preview maxBytes is too large: maximum is 40 MiB".into());
    }
    let root = Path::new(directory);
    if !root.is_absolute() {
        return Err("File preview directory must be an absolute workspace path".into());
    }
    let root = root.canonicalize().map_err(io_error)?;
    if !root.is_dir() {
        return Err("File preview directory is not a directory".into());
    }
    if path.is_empty() {
        return Err("File preview path is missing".into());
    }
    let path = root.join(path).canonicalize().map_err(io_error)?;
    if !path.starts_with(&root) {
        return Err("File preview path is outside the workspace".into());
    }
    let metadata = fs::metadata(&path).map_err(io_error)?;
    if !metadata.is_file() {
        return Err("File preview path is not a regular file".into());
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "File preview is too large: limit is {max_bytes} bytes"
        ));
    }
    let file = open(&path).map_err(io_error)?;
    // A checked pathname can be replaced before open. Validate the handle we will read,
    // without resolving its reported path again through the mutable filesystem.
    let opened_path = opened_file_path(&file)?;
    if !opened_path.is_absolute() || !opened_path.starts_with(&root) {
        return Err("File preview opened file is outside the workspace".into());
    }
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err("File preview path is not a regular file".into());
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "File preview is too large: limit is {max_bytes} bytes"
        ));
    }
    read_bounded(file, max_bytes)
}

#[cfg(windows)]
fn opened_file_path(file: &File) -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };

    // DOS + NORMALIZED uses the same extended \\?\ namespace as Path::canonicalize.
    // Bound the path buffer too; an overlong or unavailable final path fails closed.
    let mut buffer = vec![0u16; 32768];
    // SAFETY: file owns a live handle, and buffer is writable for the supplied length.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    } as usize;
    if length == 0 {
        return Err(format!(
            "File preview cannot validate opened file path: {}",
            io::Error::last_os_error()
        ));
    }
    if length >= buffer.len() {
        return Err("File preview cannot validate opened file path: path is too long".into());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

#[cfg(target_os = "linux")]
fn opened_file_path(file: &File) -> Result<PathBuf, String> {
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};

    // read_link gets the kernel's handle path; canonicalize would re-resolve its name.
    let path = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|error| format!("File preview cannot validate opened file path: {error}"))?;
    if path.as_os_str().as_bytes().ends_with(b" (deleted)") {
        return Err("File preview cannot validate opened file path: file was deleted".into());
    }
    Ok(path)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn opened_file_path(_file: &File) -> Result<PathBuf, String> {
    Err("File preview cannot validate opened-file containment on this platform".into())
}

fn read_bounded(reader: impl Read, max_bytes: u64) -> Result<FilePreview, String> {
    // The extra byte detects growth after metadata without an unbounded allocation/read.
    let mut bytes = Vec::new();
    reader
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "File preview is too large: limit is {max_bytes} bytes"
        ));
    }
    Ok(FilePreview {
        size: bytes.len() as u64,
        content: STANDARD.encode(bytes),
    })
}

fn io_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "File preview file or directory is missing".into()
    } else {
        format!("File preview could not be read: {error}")
    }
}

#[cfg(all(test, not(any(windows, target_os = "linux"))))]
#[test]
fn file_preview_handle_validation_fails_closed_on_unsupported_platforms() {
    let file = File::open(std::env::current_exe().unwrap()).unwrap();
    assert!(opened_file_path(&file)
        .unwrap_err()
        .contains("on this platform"));
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).unwrap();
            let root = std::env::temp_dir().join(format!("drift-file-preview-{random:x?}"));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("workspace")).unwrap();
            Self(root)
        }

        fn workspace(&self) -> PathBuf {
            self.0.join("workspace")
        }

        fn read(&self, path: &str, max_bytes: u64) -> Result<FilePreview, String> {
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
        assert!(fixture
            .read("missing.txt", 100)
            .unwrap_err()
            .contains("missing"));
        assert!(fixture.read("", 100).unwrap_err().contains("missing"));
        assert!(fixture
            .read(".", 100)
            .unwrap_err()
            .contains("not a regular file"));
    }

    #[test]
    fn file_preview_requires_an_absolute_existing_workspace_directory() {
        let fixture = Fixture::new();
        for directory in ["", ".", "workspace", "../workspace"] {
            assert!(read_preview("file", directory, 1)
                .unwrap_err()
                .contains("absolute"));
        }
        let missing = fixture.0.join("missing");
        assert!(read_preview("file", missing.to_str().unwrap(), 1)
            .unwrap_err()
            .contains("missing"));
        let file = fixture.0.join("file");
        fs::write(&file, b"").unwrap();
        assert!(read_preview("file", file.to_str().unwrap(), 1)
            .unwrap_err()
            .contains("not a directory"));
    }

    #[test]
    fn file_preview_rejects_oversize_metadata_and_unsafe_limits() {
        let fixture = Fixture::new();
        let path = fixture.workspace().join("large.bin");
        File::create(&path)
            .unwrap()
            .set_len(MAX_PREVIEW_BYTES + 1)
            .unwrap();
        assert!(fixture
            .read("large.bin", MAX_PREVIEW_BYTES)
            .unwrap_err()
            .contains("too large"));
        fs::write(&path, b"12").unwrap();
        for limit in [0, 1, MAX_PREVIEW_BYTES + 1, u64::MAX] {
            assert!(fixture
                .read("large.bin", limit)
                .unwrap_err()
                .contains("too large"));
        }
        assert_eq!(
            fixture.read("large.bin", MAX_PREVIEW_BYTES).unwrap().size,
            2
        );
    }

    #[test]
    fn file_preview_rejects_growth_after_metadata_with_a_bounded_read() {
        let fixture = Fixture::new();
        let path = fixture.workspace().join("growing.bin");
        fs::write(&path, b"1").unwrap();
        let file = File::open(&path).unwrap();
        let limit = file.metadata().unwrap().len();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(100)
            .unwrap();
        let mut reader = file;
        assert!(read_bounded(&mut reader, limit)
            .unwrap_err()
            .contains("too large"));
        assert_eq!(io::Seek::stream_position(&mut reader).unwrap(), limit + 1);
        assert!(read_bounded(io::repeat(0), 1)
            .unwrap_err()
            .contains("too large"));
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
            assert!(fixture
                .read(path.to_str().unwrap(), 100)
                .unwrap_err()
                .contains("outside the workspace"));
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
                "file" => (
                    fixture.workspace().join("nested/file"),
                    outside.join("file"),
                ),
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
                eprintln!(
                    "swap fixture skipped: Windows requires Developer Mode or symlink privilege"
                );
                return;
            }
            result.unwrap();

            let saved = fixture.0.join("saved");
            let mut witness = None;
            let error = read_preview_with_open(
                "nested/file",
                fixture.workspace().to_str().unwrap(),
                6,
                |path| {
                    fs::rename(&replaced, &saved)?;
                    fs::rename(&link, &replaced)?;
                    let opened = File::open(path);
                    // Restore the checked pathname before validation. Only the handle
                    // still identifies the outside file, not a fresh canonicalize(path).
                    fs::rename(&replaced, &link)?;
                    fs::rename(&saved, &replaced)?;
                    let file = opened?;
                    assert!(path
                        .canonicalize()
                        .unwrap()
                        .starts_with(fixture.workspace().canonicalize().unwrap()));
                    assert!(opened_file_path(&file)
                        .unwrap()
                        .starts_with(outside.canonicalize().unwrap()));
                    witness = Some(file.try_clone()?);
                    Ok(file)
                },
            )
            .unwrap_err();
            assert!(
                error.contains("opened file is outside the workspace"),
                "{swap}: {error}"
            );
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
            let preview =
                read_preview(path.to_str().unwrap(), directory.to_str().unwrap(), 9).unwrap();
            assert_eq!(STANDARD.decode(preview.content).unwrap(), b"long path");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_preview_rejects_deleted_opened_files() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace().join("file"), b"public").unwrap();
        let error =
            read_preview_with_open("file", fixture.workspace().to_str().unwrap(), 6, |path| {
                let file = File::open(path)?;
                fs::remove_file(path)?;
                Ok(file)
            })
            .unwrap_err();
        assert!(error.contains("cannot validate opened file path: file was deleted"));
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
            eprintln!(
                "symlink fixture skipped: Windows requires Developer Mode or symlink privilege"
            );
            return;
        }
        result.unwrap();
        assert!(fixture
            .read("escape", 100)
            .unwrap_err()
            .contains("outside the workspace"));

        let directory_link = fixture.workspace().join("outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fixture.0, &directory_link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&fixture.0, &directory_link).unwrap();
        assert!(fixture
            .read("outside/secret", 100)
            .unwrap_err()
            .contains("outside the workspace"));

        fs::write(fixture.workspace().join("inside"), b"inside").unwrap();
        let root_link = fixture.0.join("root-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(fixture.workspace(), &root_link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(fixture.workspace(), &root_link).unwrap();
        assert_eq!(
            read_preview("inside", root_link.to_str().unwrap(), 6)
                .unwrap()
                .size,
            6
        );
    }

    #[tokio::test]
    async fn file_preview_async_command_returns_serializable_bytes() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace().join("file"), b"\0\xff \n").unwrap();
        let preview = read_file_preview(
            "file".into(),
            fixture.workspace().to_str().unwrap().into(),
            4,
        )
        .await
        .unwrap();
        assert_eq!(preview.size, 4);
        assert_eq!(STANDARD.decode(preview.content).unwrap(), b"\0\xff \n");
    }
}
