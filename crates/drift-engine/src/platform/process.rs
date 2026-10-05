//! Killing a shell must kill what it started. Windows uses a job object, unix a process group.

use std::io;

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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

        pub fn is_empty(&self) -> io::Result<bool> {
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let size = std::mem::size_of_val(&info) as u32;
                if QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(), size, std::ptr::null_mut()) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(info.ActiveProcesses == 0)
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

    pub fn suspend(command: &mut tokio::process::Command) {
        command.creation_flags(0x0800_0000 | 0x0000_0004);
    }

    pub fn resume(child: &tokio::process::Child) -> io::Result<()> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD};
        let pid = child.id().ok_or_else(|| io::Error::other("suspended child has no process ID"))?;
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
        let result = resume_thread(snapshot, pid);
        unsafe { CloseHandle(snapshot); }
        result
    }

    fn resume_thread(snapshot: HANDLE, pid: u32) -> io::Result<()> {
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{Thread32First, Thread32Next, THREADENTRY32};
        use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
        unsafe {
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            if Thread32First(snapshot, &mut entry) == 0 { return Err(io::Error::last_os_error()); }
            loop {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if thread.is_null() { return Err(io::Error::last_os_error()); }
                    let resumed = ResumeThread(thread);
                    let error = io::Error::last_os_error();
                    CloseHandle(thread);
                    return if resumed == u32::MAX { Err(error) } else { Ok(()) };
                }
                if Thread32Next(snapshot, &mut entry) == 0 {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "suspended child thread not found"));
                }
            }
        }
    }
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

        pub fn is_empty(&self) -> io::Result<bool> {
            if unsafe { libc::kill(-self.0, 0) } == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) { return Ok(true); }
                return Err(error);
            }
            #[cfg(target_os = "linux")]
            return linux_group_empty(self.0);
            #[cfg(not(target_os = "linux"))]
            Ok(false)
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

    pub fn suspend(_command: &mut tokio::process::Command) {}

    pub fn resume(_child: &tokio::process::Child) -> io::Result<()> { Ok(()) }

    #[cfg(target_os = "linux")]
    fn linux_group_empty(group: i32) -> io::Result<bool> {
        for entry in std::fs::read_dir("/proc")? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().parse::<u32>().is_err() { continue; }
            let stat = match std::fs::read_to_string(entry.path().join("stat")) {
                Ok(stat) => stat,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if group_running(&stat, group) { return Ok(false); }
        }
        Ok(true)
    }

    #[cfg(target_os = "linux")]
    fn group_running(stat: &str, group: i32) -> bool {
        let Some((_, tail)) = stat.rsplit_once(") ") else { return false };
        let mut fields = tail.split_whitespace();
        let state = fields.next();
        fields.next();
        let process_group = fields.next().and_then(|id| id.parse::<i32>().ok());
        process_group == Some(group) && !matches!(state, Some("Z" | "X"))
    }
}

pub use imp::{prepare, Tree};

impl Tree {
    /// Returns only after the owned tree has no remaining writers; failed queries never permit cleanup to proceed.
    pub async fn stop(&self) {
        let mut reported = false;
        loop {
            self.kill();
            match self.is_empty() {
                Ok(true) => return,
                Err(error) if !reported => {
                    eprintln!("could not confirm process-tree termination: {error}; retaining writer locks and retrying");
                    reported = true;
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

/// Starts a child only after establishing tree ownership; failures kill and reap the suspended child.
pub async fn spawn_owned(command: &mut tokio::process::Command) -> io::Result<(tokio::process::Child, Tree)> {
    spawn_with(command, Tree::adopt).await
}

async fn spawn_with(command: &mut tokio::process::Command, adopt: impl FnOnce(u32) -> io::Result<Tree>) -> io::Result<(tokio::process::Child, Tree)> {
    command.kill_on_drop(true);
    prepare(command);
    imp::suspend(command);
    let mut child = command.spawn()?;
    let owned = child.id().ok_or_else(|| io::Error::other("child has no process ID")).and_then(adopt);
    let tree = match owned {
        Ok(tree) => tree,
        Err(error) => { let _ = child.kill().await; return Err(error); }
    };
    if let Err(error) = imp::resume(&child) {
        tree.kill();
        let _ = child.kill().await;
        drop(child);
        tree.stop().await;
        return Err(error);
    }
    Ok((child, tree))
}

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

pub fn find_in(program: &str, dirs: impl Iterator<Item = std::path::PathBuf>) -> Option<std::path::PathBuf> {
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
    #[cfg(windows)]
    #[tokio::test]
    async fn failed_ownership_never_resumes_the_child() {
        let root = std::env::temp_dir().join(format!("drift-owned-fail-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("ran.txt");
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/c", "echo ran>ran.txt"]).current_dir(&root);
        let failure = super::spawn_with(&mut command, |_| Err(std::io::Error::other("injected job adoption failure"))).await;
        assert!(failure.is_err());
        assert!(!marker.exists(), "no code ran before adoption failed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn tree_cleanup_waits_for_a_descendant_after_its_parent_exits() {
        use std::process::Stdio;
        let root = std::env::temp_dir().join(format!("drift-owned-tree-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("parent.cmd"), "@echo off\r\nstart \"\" /b cmd.exe /c \"ping -n 30 127.0.0.1 > nul & echo late>late.txt\"\r\nexit /b 0\r\n").unwrap();
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/c", "parent.cmd"]).current_dir(&root).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let (mut child, tree) = super::spawn_owned(&mut command).await.unwrap();
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await.unwrap().unwrap();
        assert!(status.success());
        drop(child);
        assert!(!tree.is_empty().unwrap(), "the exited parent left a descendant in the job");
        tokio::time::timeout(std::time::Duration::from_secs(5), tree.stop()).await.expect("all job members terminated");
        assert!(tree.is_empty().unwrap());
        assert!(!root.join("late.txt").exists());
        drop(tree);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Err(error) = std::fs::remove_dir_all(&root) {
            assert!(tokio::time::Instant::now() < deadline, "temporary directory cleanup failed: {error}");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

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
