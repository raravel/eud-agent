#[cfg(windows)]
pub(crate) struct WindowsJob {
    job: isize,
    primary_process: Option<isize>,
}
#[cfg(windows)]
impl WindowsJob {
    const REAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const REAP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);

    pub(crate) fn assign(child: &tokio::process::Child) -> std::io::Result<Self> {
        use windows_sys::Win32::Foundation::HANDLE;
        let child_handle = child
            .raw_handle()
            .ok_or_else(std::io::Error::last_os_error)?;
        Self::assign_handle(child_handle as HANDLE, None)
    }

    /// Contain a std child; `process_memory_limit` additionally caps the
    /// committed memory of each process in the job.
    pub(crate) fn assign_std_with_memory_limit(
        child: &std::process::Child,
        process_memory_limit: Option<usize>,
    ) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        Self::assign_handle(child.as_raw_handle() as HANDLE, process_memory_limit)
    }

    fn assign_handle(
        child_handle: windows_sys::Win32::Foundation::HANDLE,
        process_memory_limit: Option<usize>,
    ) -> std::io::Result<Self> {
        use std::mem::size_of;
        use windows_sys::Win32::Foundation::{
            CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE,
        };
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: Category 8 (FFI). Null names are accepted by CreateJobObjectW, and
        // the returned handle is checked before use and owned by WindowsJob.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: Category 4 (uninitialized memory). The Windows structure permits
        // zero initialization and every field read by this call is initialized below.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(limit) = process_memory_limit {
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = limit;
        }
        // SAFETY: Category 8 (FFI). `handle` is valid and `info` remains alive with
        // the exact Windows-declared byte size for the duration of the call.
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        let assigned = if configured != 0 {
            // SAFETY: Category 8 (FFI). Both handles are live OS handles and neither
            // is closed while AssignProcessToJobObject executes.
            unsafe { AssignProcessToJobObject(handle, child_handle) }
        } else {
            0
        };
        if configured == 0 || assigned == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: Category 12 (invalid/double free). This error branch owns the
            // checked non-null handle and returns before WindowsJob can own it.
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        let mut primary_process: HANDLE = std::ptr::null_mut();
        // SAFETY: Category 8 (FFI). The child still owns `child_handle`; duplicating
        // it in this process preserves its existing synchronization access.
        let duplicated = unsafe {
            let current_process = GetCurrentProcess();
            DuplicateHandle(
                current_process,
                child_handle,
                current_process,
                &raw mut primary_process,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: Category 12 (invalid/double free). This branch still owns
            // the job handle and returns before WindowsJob can own it.
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self {
            job: handle as isize,
            primary_process: Some(primary_process as isize),
        })
    }
    pub(crate) fn terminate(self) {
        drop(self);
    }

    fn terminate_and_reap(&mut self) -> std::io::Result<()> {
        use std::mem::size_of;
        use std::time::Instant;
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject, TerminateJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let handle = self.job as HANDLE;
        // SAFETY: Category 8 (FFI). WindowsJob exclusively owns this still-live job
        // handle for this call and the subsequent accounting queries.
        let terminated = unsafe { TerminateJobObject(handle, 1) };
        let termination_error = (terminated == 0).then(std::io::Error::last_os_error);
        let started = Instant::now();
        let primary_process = self.primary_process.take().ok_or_else(|| {
            std::io::Error::other("Windows provider primary process was already reaped")
        })?;
        // SAFETY: Category 8 (FFI). WindowsJob exclusively owns the duplicated
        // SYNCHRONIZE handle and keeps it live for the duration of this wait.
        let wait_result = unsafe {
            WaitForSingleObject(
                primary_process as HANDLE,
                u32::try_from(Self::REAP_TIMEOUT.as_millis()).unwrap_or(u32::MAX),
            )
        };
        let wait_error = (wait_result == WAIT_FAILED).then(std::io::Error::last_os_error);
        // SAFETY: Category 12 (invalid/double free). `take` transferred the sole
        // duplicated-handle ownership here, so no later Drop path can close it.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(primary_process as HANDLE) };
        if let Some(error) = wait_error {
            return Err(error);
        }
        match wait_result {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Windows provider primary process did not terminate",
                ));
            }
            result => {
                return Err(std::io::Error::other(format!(
                    "unexpected Windows provider process wait result {result}"
                )));
            }
        }
        loop {
            // SAFETY: Category 4 (uninitialized memory). The Windows accounting
            // structure permits zero initialization and the query initializes it.
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            // SAFETY: Category 8 (FFI). `handle` remains live, `accounting` is a
            // writable Windows-declared structure, and its exact size is supplied.
            let queried = unsafe {
                QueryInformationJobObject(
                    handle,
                    JobObjectBasicAccountingInformation,
                    (&raw mut accounting).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if queried == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if accounting.ActiveProcesses == 0 {
                return termination_error.map_or(Ok(()), Err);
            }
            if started.elapsed() >= Self::REAP_TIMEOUT {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "Windows provider job still has {} active process(es) after termination",
                        accounting.ActiveProcesses
                    ),
                ));
            }
            std::thread::sleep(Self::REAP_POLL_INTERVAL);
        }
    }
}
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        if let Err(error) = self.terminate_and_reap() {
            eprintln!("eud-agent: Windows provider job cleanup failed: {error}");
        }
        if let Some(primary_process) = self.primary_process.take() {
            // SAFETY: Category 12 (invalid/double free). This handle remains only
            // when reaping returned before consuming it, and `take` makes closing unique.
            unsafe { CloseHandle(primary_process as HANDLE) };
        }
        // SAFETY: Category 12 (invalid/double free). Drop runs once for the sole
        // WindowsJob owner and this checked job handle remains live until here.
        unsafe { CloseHandle(self.job as HANDLE) };
    }
}
#[cfg(not(windows))]
pub(crate) struct WindowsJob;
#[cfg(not(windows))]
impl WindowsJob {
    pub(crate) fn assign(_child: &tokio::process::Child) -> std::io::Result<Self> {
        Ok(Self)
    }
    pub(crate) fn terminate(self) {}
}
