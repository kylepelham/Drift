//! Killing a shell must kill what it started. Windows uses a job object, unix a process group.

use std::io;

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// Every descendant of the child lives in this job; closing it, however that happens, kills them all.
    pub struct Tree(HANDLE);

    unsafe impl Send for Tree {}
    unsafe impl Sync for Tree {}

    impl Tree {
        pub fn adopt(pid: u32) -> io::Result<Self> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                if SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size) == 0 {
                    CloseHandle(job);
                    return Err(io::Error::last_os_error());
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    CloseHandle(job);
                    return Err(io::Error::last_os_error());
                }
                let assigned = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if assigned == 0 {
                    CloseHandle(job);
                    return Err(io::Error::last_os_error());
                }
                Ok(Self(job))
            }
        }

        pub fn kill(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn prepare(_command: &mut tokio::process::Command) {}
}

#[cfg(unix)]
mod imp {
    use super::*;

    /// The child leads its own process group; a signal to the group reaches every descendant.
    pub struct Tree(i32);

    impl Tree {
        pub fn adopt(pid: u32) -> io::Result<Self> {
            Ok(Self(pid as i32))
        }

        pub fn kill(&self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            self.kill();
        }
    }

    pub fn prepare(command: &mut tokio::process::Command) {
        command.process_group(0);
    }
}

pub use imp::{prepare, Tree};

/// Where a program named without a path is found on PATH, as a shell would, `.cmd` and `.bat` shims included on Windows.
pub fn which(program: &str) -> Option<std::path::PathBuf> {
    find_in(program, std::env::split_paths(&current_path()))
}

/// The PATH a program started now should get: Drift's own, then directories added since Drift started, as an install does.
pub fn current_path() -> std::ffi::OsString {
    merged(&std::env::var_os("PATH").unwrap_or_default(), saved_path())
}

/// Gives a command the current PATH, unless its own environment already sets one.
pub fn use_current_path(command: &mut tokio::process::Command, own: &std::collections::BTreeMap<String, String>) {
    if !own.keys().any(|name| name.eq_ignore_ascii_case("PATH")) {
        command.env("PATH", current_path());
    }
}

fn merged(live: &std::ffi::OsStr, saved: Option<std::ffi::OsString>) -> std::ffi::OsString {
    let key = |dir: &std::path::Path| dir.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(live).collect();
    let mut seen: std::collections::HashSet<String> = dirs.iter().map(|dir| key(dir)).collect();
    for dir in saved.iter().flat_map(std::env::split_paths) {
        if !dir.as_os_str().is_empty() && seen.insert(key(&dir)) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| live.to_os_string())
}

/// The user's then the machine's PATH as Windows keeps them now, placeholders expanded.
#[cfg(windows)]
fn saved_path() -> Option<std::ffi::OsString> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let user = registry_string(HKEY_CURRENT_USER, "Environment", "Path");
    let machine = registry_string(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", "Path");
    let joined: Vec<String> = [user, machine].into_iter().flatten().collect();
    (!joined.is_empty()).then(|| joined.join(";").into())
}

#[cfg(not(windows))]
fn saved_path() -> Option<std::ffi::OsString> {
    None
}

#[cfg(windows)]
fn registry_string(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide(key), wide(value));
    let mut size = 0u32;
    // A REG_EXPAND_SZ value comes back expanded, as RRF_RT_REG_SZ without RRF_NOEXPAND asks.
    let sized = unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
    if sized != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let mut written = (buffer.len() * 2) as u32;
    let read = unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut written) };
    if read != 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..written as usize / 2]);
    Some(text.trim_end_matches('\0').to_string())
}

fn find_in(program: &str, dirs: impl Iterator<Item = std::path::PathBuf>) -> Option<std::path::PathBuf> {
    let named = std::path::Path::new(program);
    if named.components().count() > 1 {
        return named.is_file().then(|| named.to_path_buf());
    }
    let names: Vec<String> = if cfg!(windows) && named.extension().is_none() {
        ["exe", "cmd", "bat"].iter().map(|ext| format!("{program}.{ext}")).collect()
    } else {
        vec![program.to_string()]
    };
    dirs.flat_map(|dir| names.iter().map(move |name| dir.join(name)).collect::<Vec<_>>()).find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_program_is_found_where_a_shell_would_find_it() {
        let dir = std::env::temp_dir().join(format!("drift-which-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = if cfg!(windows) { "lint.cmd" } else { "lint" };
        std::fs::write(dir.join(shim), "").unwrap();
        assert_eq!(super::find_in("lint", [dir.join("missing"), dir.clone()].into_iter()), Some(dir.join(shim)));
        assert_eq!(super::find_in("absent", [dir.clone()].into_iter()), None);
        let full = dir.join(shim).to_string_lossy().to_string();
        assert_eq!(super::find_in(&full, std::iter::empty()), Some(dir.join(shim)), "a path is taken as given");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn directories_added_since_start_follow_the_ones_drift_started_with() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let live = ["/a", "/b/"].join(sep);
        let saved = ["/B", "/c", "", "/a"].join(sep);
        let merged = super::merged(std::ffi::OsStr::new(&live), Some(saved.into()));
        let dirs: Vec<_> = std::env::split_paths(&merged).map(|dir| dir.to_string_lossy().into_owned()).collect();
        assert_eq!(dirs, ["/a", "/b/", "/c"], "kept in order, nothing twice, case and trailing slash aside");
    }

    #[cfg(windows)]
    #[test]
    fn the_saved_path_is_read_from_the_registry() {
        let saved = super::saved_path().expect("Windows keeps a PATH");
        assert!(std::env::split_paths(&saved).any(|dir| dir.join("cmd.exe").is_file() || dir.join("System32").is_dir()), "{saved:?}");
    }
}
