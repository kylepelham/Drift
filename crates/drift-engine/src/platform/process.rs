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
