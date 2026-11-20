//! Own only the worker's process tree; never terminate unrelated CLI sessions.
use tokio::process::{Child, Command};

pub fn configure(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;
    type Handle = *mut c_void;
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: Handle, name: *const u16) -> Handle;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            info: *const c_void,
            length: u32,
        ) -> i32;
    }
    pub struct Tree(Handle);
    // An owned Windows kernel handle can be moved between runtime threads.
    unsafe impl Send for Tree {}
    impl Tree {
        pub fn attach(child: &Child) -> Result<Self, String> {
            let process = child.raw_handle().ok_or("工作进程句柄不可用")?;
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Err("无法创建工作进程组".into());
                }
                let limits = ExtendedLimits {
                    basic: BasicLimits {
                        flags: 0x2000,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE also covers app crashes.
                if SetInformationJobObject(
                    job,
                    9,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                    || AssignProcessToJobObject(job, process as Handle) == 0
                {
                    CloseHandle(job);
                    return Err("无法监管工作进程组，已拒绝执行".into());
                }
                Ok(Self(job))
            }
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            unsafe {
                TerminateJobObject(self.0, 1);
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    pub struct Tree(i32);
    impl Tree {
        pub fn attach(child: &Child) -> Result<Self, String> {
            child
                .id()
                .map(|id| Self(id as i32))
                .ok_or_else(|| "工作进程标识不可用".into())
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            unsafe {
                kill(-self.0, 9);
            }
        }
    }
}
pub use platform::Tree;
