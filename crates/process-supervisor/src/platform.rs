//! The only platform FFI in this crate. Each unsafe call is kept next to the
//! OS invariant that makes it valid.

use std::io;
use std::process::{Child, Command};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PlatformCounters {
    pub(crate) cpu_time: Option<Duration>,
    pub(crate) peak_memory_bytes: Option<u64>,
}

#[cfg(unix)]
mod imp {
    use std::os::unix::process::CommandExt;

    use super::*;

    pub(crate) struct ProcessTree {
        pgid: i32,
    }

    pub(crate) fn prepare_command(command: &mut Command) {
        // A zero process_group asks `posix_spawn`/`setpgid` to use the child PID.
        command.process_group(0);
    }

    pub(crate) fn resume_process(_pid: u32) -> io::Result<()> {
        Ok(())
    }

    impl ProcessTree {
        pub(crate) fn attach_std(child: &Child) -> io::Result<Self> {
            Self::from_pid(child.id())
        }

        pub(crate) fn attach_pty(pid: u32) -> io::Result<Self> {
            // portable-pty creates a controlling terminal session whose leader is
            // the spawned process, so its PID is also its process-group ID.
            Self::from_pid(pid)
        }

        fn from_pid(pid: u32) -> io::Result<Self> {
            let pgid = i32::try_from(pid)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID exceeds i32"))?;
            Ok(Self { pgid })
        }

        pub(crate) fn graceful(&self) -> io::Result<()> {
            self.signal(libc::SIGTERM)
        }

        pub(crate) fn terminate(&self) -> io::Result<()> {
            self.signal(libc::SIGKILL)
        }

        fn signal(&self, signal: i32) -> io::Result<()> {
            // SAFETY: `self.pgid` comes from a successfully spawned child and is
            // positive. Negating it addresses that child's process group only.
            if unsafe { libc::kill(-self.pgid, signal) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            }
        }

        pub(crate) fn counters(&self) -> PlatformCounters {
            PlatformCounters::default()
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            let _ = self.terminate();
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, RawHandle};
    use std::os::windows::process::CommandExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    use super::*;

    pub(crate) struct ProcessTree {
        // Stored as an integer so ownership can cross threads; Win32 kernel
        // handles are process-wide values and CloseHandle is thread-agnostic.
        handle: usize,
    }

    pub(crate) fn prepare_command(command: &mut Command) {
        command.creation_flags(CREATE_SUSPENDED);
    }

    pub(crate) fn resume_process(pid: u32) -> io::Result<()> {
        // SAFETY: this creates an owned snapshot handle, checked below and closed
        // exactly once before returning.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let result = resume_process_threads(snapshot, pid);
        // SAFETY: `snapshot` is the live owned handle created above.
        unsafe {
            CloseHandle(snapshot);
        }
        result
    }

    fn resume_process_threads(snapshot: HANDLE, pid: u32) -> io::Result<()> {
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: `entry` is writable and advertises the required structure size;
        // `snapshot` is a live thread snapshot for this entire enumeration.
        if unsafe { Thread32First(snapshot, &raw mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut found = false;
        loop {
            if entry.th32OwnerProcessID == pid {
                resume_thread(entry.th32ThreadID)?;
                found = true;
            }
            // SAFETY: the same initialized entry and live snapshot remain valid.
            if unsafe { Thread32Next(snapshot, &raw mut entry) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                    return Err(error);
                }
                break;
            }
        }
        found.then_some(()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "suspended process thread was not found",
            )
        })
    }

    fn resume_thread(thread_id: u32) -> io::Result<()> {
        // SAFETY: OpenThread validates the enumerated thread ID. The returned
        // borrowed-kernel-object handle is checked and closed exactly once.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if thread.is_null() {
            return Err(io::Error::last_os_error());
        }
        let result = loop {
            // SAFETY: `thread` is a live handle with THREAD_SUSPEND_RESUME access.
            let previous_count = unsafe { ResumeThread(thread) };
            if previous_count == u32::MAX {
                break Err(io::Error::last_os_error());
            }
            if previous_count <= 1 {
                break Ok(());
            }
        };
        // SAFETY: this is the single close for the handle returned by OpenThread.
        unsafe {
            CloseHandle(thread);
        }
        result
    }

    impl ProcessTree {
        pub(crate) fn attach_std(child: &Child) -> io::Result<Self> {
            Self::attach(child.as_raw_handle())
        }

        pub(crate) fn attach_pty(raw_handle: RawHandle) -> io::Result<Self> {
            Self::attach(raw_handle)
        }

        fn attach(process: RawHandle) -> io::Result<Self> {
            // SAFETY: null security attributes and name request a new unnamed job;
            // the returned handle is checked and owned by `Self` on success.
            let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let tree = Self {
                handle: job as usize,
            };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` has the exact layout and byte size required for
            // JobObjectExtendedLimitInformation, and the job handle is live.
            let configured = unsafe {
                SetInformationJobObject(
                    tree.raw(),
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: both handles are live. The process handle is borrowed from
            // the child and remains live for the duration of this call.
            if unsafe { AssignProcessToJobObject(tree.raw(), process.cast()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(tree)
        }

        fn raw(&self) -> HANDLE {
            self.handle as HANDLE
        }

        pub(crate) fn graceful(&self) -> io::Result<()> {
            // Windows has no reliable provider-neutral graceful signal for GUI,
            // console, and ConPTY children. `Supervisor::stop` still waits for
            // the grace interval before terminating the job.
            Ok(())
        }

        pub(crate) fn terminate(&self) -> io::Result<()> {
            // SAFETY: the handle is a live job owned by `Self`.
            if unsafe { TerminateJobObject(self.raw(), 1) } == 0 {
                let error = io::Error::last_os_error();
                // ERROR_ACCESS_DENIED is returned after the job has already ended.
                if error.raw_os_error() == Some(5) {
                    Ok(())
                } else {
                    Err(error)
                }
            } else {
                Ok(())
            }
        }

        pub(crate) fn counters(&self) -> PlatformCounters {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: `accounting` is writable for the exact size supplied and
            // matches JobObjectBasicAccountingInformation.
            let accounting_ok = unsafe {
                QueryInformationJobObject(
                    self.raw(),
                    JobObjectBasicAccountingInformation,
                    (&raw mut accounting).cast::<c_void>(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    ptr::null_mut(),
                )
            } != 0;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // SAFETY: `limits` is writable for the exact size supplied and
            // matches JobObjectExtendedLimitInformation.
            let limits_ok = unsafe {
                QueryInformationJobObject(
                    self.raw(),
                    JobObjectExtendedLimitInformation,
                    (&raw mut limits).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    ptr::null_mut(),
                )
            } != 0;

            PlatformCounters {
                cpu_time: accounting_ok.then(|| {
                    let ticks = accounting
                        .TotalUserTime
                        .saturating_add(accounting.TotalKernelTime);
                    Duration::from_nanos(u64::try_from(ticks).unwrap_or(0).saturating_mul(100))
                }),
                peak_memory_bytes: limits_ok.then_some(limits.PeakJobMemoryUsed as u64),
            }
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            // SAFETY: this is the single CloseHandle for the job owned by `Self`.
            unsafe {
                CloseHandle(self.raw());
            }
        }
    }
}

pub(crate) use imp::{ProcessTree, prepare_command, resume_process};
