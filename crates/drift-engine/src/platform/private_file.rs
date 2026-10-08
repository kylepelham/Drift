//! Owner-restricted atomic persistence for credential files; only the file is restricted, never the data directory around it.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("file has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = Temporary(parent.join(format!(".credentials-{}.tmp", crate::random_hex(16))));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&temporary.0)?;
    restrict(&temporary.0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    replace(&temporary.0, path)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

#[cfg(unix)]
fn replace(from: &Path, to: &Path) -> io::Result<()> { std::fs::rename(from, to) }

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let wide = |path: &Path| path.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (from, to) = (wide(from), wide(to));
    for attempt in 0..20 {
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } != 0 { return Ok(()); }
        let error = io::Error::last_os_error();
        if attempt == 19 || !matches!(error.raw_os_error(), Some(5 | 32 | 33)) { return Err(error); }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    unreachable!("bounded replacement loop always returns")
}

#[cfg(unix)]
pub fn restrict(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
pub fn restrict(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION};
    let descriptor = format!("D:P(A;;FA;;;{})(A;;FA;;;SY)", user_sid()?);
    let descriptor: Vec<u16> = descriptor.encode_utf16().chain([0]).collect();
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut security = std::ptr::null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(descriptor.as_ptr(), 1, &mut security, std::ptr::null_mut()) == 0 { return Err(io::Error::last_os_error()); }
        let set = SetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, security);
        let error = io::Error::last_os_error();
        LocalFree(security);
        if set == 0 { return Err(error); }
    }
    Ok(())
}

#[cfg(windows)]
fn user_sid() -> io::Result<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = std::ptr::null_mut();
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 { return Err(io::Error::last_os_error()); }
        let result = token_sid(token);
        CloseHandle(token);
        result
    }
}

#[cfg(windows)]
unsafe fn token_sid(token: windows_sys::Win32::Foundation::HANDLE) -> io::Result<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_USER};

    // SAFETY: the caller passes an open token handle; the buffer is sized by the first call and aligned for
    // TOKEN_USER, and the SID string is read up to its NUL terminator before LocalFree releases it.
    unsafe {
        let mut length = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length);
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), length, &mut length) == 0 {
            return Err(io::Error::last_os_error());
        }

        let sid = (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let mut text = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut count = 0;
        while *text.add(count) != 0 {
            count += 1;
        }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(text, count));
        LocalFree(text.cast());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_replacement_preserves_the_destination_and_removes_the_temporary_file() {
        let dir = std::env::temp_dir().join(format!("drift-private-file-{}", crate::random_hex(4)));
        let destination = dir.join("destination");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("original"), "kept").unwrap();
        assert!(write(&destination, b"new bytes").is_err());
        assert_eq!(std::fs::read_to_string(destination.join("original")).unwrap(), "kept");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn credential_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("drift-private-mode-{}", crate::random_hex(4)));
        write(&dir.join("secret"), b"secret").unwrap();
        let before = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        write(&dir.join("secret"), b"again").unwrap();
        assert_eq!(std::fs::metadata(dir.join("secret")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, before, "the directory is left as it was");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn credential_files_have_a_protected_two_entry_dacl() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Security::{GetFileSecurityW, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED};
        let dir = std::env::temp_dir().join(format!("drift-private-dacl-{}", crate::random_hex(4)));
        let path = dir.join("secret");
        write(&path, b"secret").unwrap();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        unsafe {
            let mut length = 0;
            GetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, std::ptr::null_mut(), 0, &mut length);
            let mut bytes = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
            let descriptor = bytes.as_mut_ptr().cast();
            assert_ne!(GetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, descriptor, length, &mut length), 0);
            let (mut present, mut defaulted, mut acl) = (0, 0, std::ptr::null_mut());
            assert_ne!(GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted), 0);
            assert_ne!(present, 0);
            assert_eq!((*acl).AceCount, 2);
            let (mut control, mut revision) = (0, 0);
            assert_ne!(GetSecurityDescriptorControl(descriptor, &mut control, &mut revision), 0);
            assert_ne!(control & SE_DACL_PROTECTED, 0);
            let folder: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
            let mut length = 0;
            GetFileSecurityW(folder.as_ptr(), DACL_SECURITY_INFORMATION, std::ptr::null_mut(), 0, &mut length);
            let mut bytes = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
            let descriptor = bytes.as_mut_ptr().cast();
            assert_ne!(GetFileSecurityW(folder.as_ptr(), DACL_SECURITY_INFORMATION, descriptor, length, &mut length), 0);
            let (mut control, mut revision) = (0, 0);
            assert_ne!(GetSecurityDescriptorControl(descriptor, &mut control, &mut revision), 0);
            assert_eq!(control & SE_DACL_PROTECTED, 0, "the directory keeps its inherited access");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
