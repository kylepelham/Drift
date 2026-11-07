use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED, ERROR_SHARING_VIOLATION,
    ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, ERROR_UNABLE_TO_REMOVE_REPLACED,
};
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// A file another program holds open is usually released within moments, so it is tried a few times.
const ATTEMPTS: u32 = 5;
const PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

pub(super) fn replace_file(path: &Path, staged: &Path, backup: &Path) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match replace_once(path, staged, backup) {
            Err(error) if code(&error) == ERROR_SHARING_VIOLATION && attempt < ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(PAUSE);
            }
            // Network shares such as WSL's cannot swap files in one step; a plain move still replaces it.
            Err(error) if [ERROR_NOT_SUPPORTED, ERROR_INVALID_FUNCTION].contains(&code(&error)) => {
                return std::fs::rename(staged, path);
            }
            done => return explained(done),
        }
    }
}

fn code(error: &io::Error) -> u32 {
    error.raw_os_error().unwrap_or_default() as u32
}

fn replace_once(path: &Path, staged: &Path, backup: &Path) -> io::Result<()> {
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
    // The original was moved to the backup name but the new file could not take its place: move it back.
    if code(&error) == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
        let _ = std::fs::rename(backup, path);
    }
    Err(error)
}

fn explained(done: io::Result<()>) -> io::Result<()> {
    let Err(error) = done else {
        return Ok(());
    };
    if [
        ERROR_SHARING_VIOLATION,
        ERROR_ACCESS_DENIED,
        ERROR_UNABLE_TO_REMOVE_REPLACED,
    ]
    .contains(&code(&error))
    {
        return Err(io::Error::new(
            error.kind(),
            format!("{error} (another program may have it open without allowing it to be replaced)"),
        ));
    }
    Err(error)
}
