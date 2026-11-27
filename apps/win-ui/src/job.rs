//! Windows Job Object wrapper ensuring spawned child processes (such as the
//! host worker) are automatically terminated when the parent exits or crashes.
//!
//! Uses `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. On non-Windows platforms this
//! module compiles as a clean no-op stub.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Owns a Windows Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    pub struct ChildJob {
        raw: isize,
    }

    impl ChildJob {
        /// Creates a new anonymous Job Object that kills all assigned processes on close.
        pub fn new() -> Option<Self> {
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok()?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let ok = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };

            if ok.is_err() {
                unsafe {
                    let _ = CloseHandle(job);
                }
                return None;
            }

            Some(Self {
                raw: job.0 as isize,
            })
        }

        /// Assigns a running child process to this Job Object.
        pub fn assign_child(&self, child: &Child) -> bool {
            let job = HANDLE(self.raw as *mut c_void);
            let proc_handle = HANDLE(child.as_raw_handle());
            unsafe { AssignProcessToJobObject(job, proc_handle) }.is_ok()
        }
    }

    impl Drop for ChildJob {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(HANDLE(self.raw as *mut c_void));
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::process::Child;

    /// Cross-platform no-op stub on non-Windows targets.
    pub struct ChildJob {
        _priv: (),
    }

    impl ChildJob {
        pub fn new() -> Option<Self> {
            None
        }

        pub fn assign_child(&self, _child: &Child) -> bool {
            false
        }
    }
}

pub use imp::ChildJob;
