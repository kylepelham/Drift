use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, ERROR_UNABLE_TO_REMOVE_REPLACED,
};
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

pub(super) fn replace_file(path: &Path, staged: &Path, backup: &Path) -> io::Result<()> {
    let (replaced, replacement, saved) = (wide(path), wide(staged), wide(backup));
    // SAFETY: three NUL-terminated paths that outlive the call; the reserved pointers are null.
    let done = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            saved.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if done != 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    let code = error.raw_os_error().unwrap_or_default() as u32;
    // The original was moved to the backup name but the new file could not take its place: move it back.
    if code == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
        let _ = std::fs::rename(backup, path);
    }

    if [
        ERROR_SHARING_VIOLATION,
        ERROR_ACCESS_DENIED,
        ERROR_UNABLE_TO_REMOVE_REPLACED,
    ]
    .contains(&code)
    {
        return Err(io::Error::new(
            error.kind(),
            format!("{error} (another program may have it open without allowing it to be replaced)"),
        ));
    }
    Err(error)
}
