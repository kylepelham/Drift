use std::fs::File;
use std::path::PathBuf;

use super::PreviewError;

#[cfg(windows)]
pub(super) fn opened_file_path(file: &File) -> Result<PathBuf, PreviewError> {
    use std::ffi::OsString;
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    // DOS + NORMALIZED matches Path::canonicalize's extended \\?\ namespace without resolving the pathname again.
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
        return Err(PreviewError::OpenedPath(std::io::Error::last_os_error()));
    }
    if length >= buffer.len() {
        return Err(PreviewError::PathTooLong);
    }

    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

#[cfg(target_os = "linux")]
pub(super) fn opened_file_path(file: &File) -> Result<PathBuf, PreviewError> {
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};

    // read_link gets the kernel's handle path; canonicalize would resolve the mutable pathname again.
    let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(PreviewError::OpenedPath)?;
    if path.as_os_str().as_bytes().ends_with(b" (deleted)") {
        return Err(PreviewError::Deleted);
    }

    Ok(path)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(super) fn opened_file_path(_file: &File) -> Result<PathBuf, PreviewError> {
    Err(PreviewError::UnsupportedPlatform)
}
