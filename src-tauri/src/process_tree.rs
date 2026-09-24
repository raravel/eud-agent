//! Process-tree runner used by managed Python tooling and euddraft.
//!
//! Windows children are created suspended, assigned to a non-breakaway Job Object
//! configured with kill-on-close, and resumed only after stdout/stderr drainers are
//! active. This closes the spawn/assignment race and guarantees timeout/cancellation
//! terminates descendants as one unit. Unix children lead a new process group that
//! is signalled as one unit on timeout, cancellation, and root exit.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Default)]
pub struct ProcessCancellation(Arc<AtomicBool>);

impl ProcessCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEnd {
    Exited(u32),
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub end: ProcessEnd,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ProcessOutput {
    pub fn success(&self) -> bool {
        self.end == ProcessEnd::Exited(0)
    }

    pub fn exit_code(&self) -> Option<u32> {
        match self.end {
            ProcessEnd::Exited(code) => Some(code),
            ProcessEnd::TimedOut | ProcessEnd::Cancelled => None,
        }
    }

    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Run a command with bounded lifetime and concurrent output capture.
///
/// On Windows the child and every descendant are held by one kill-on-close Job
/// Object. A cancellation already set before the call prevents spawning.
pub fn run_process_tree(
    mut command: Command,
    timeout: Duration,
    cancellation: Option<&ProcessCancellation>,
) -> Result<ProcessOutput, String> {
    if cancellation.is_some_and(ProcessCancellation::is_cancelled) {
        return Ok(ProcessOutput {
            end: ProcessEnd::Cancelled,
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    platform::configure_suspended(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("프로세스를 시작하지 못했습니다: {error}"))?;

    let guard = match platform::ProcessTreeGuard::attach(&child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "프로세스 stdout 파이프를 열지 못했습니다.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "프로세스 stderr 파이프를 열지 못했습니다.".to_string())?;
    let stdout_drain = thread::spawn(move || drain(stdout));
    let stderr_drain = thread::spawn(move || drain(stderr));

    if let Err(error) = guard.resume() {
        guard.terminate(1);
        let _ = child.wait();
        let _ = stdout_drain.join();
        let _ = stderr_drain.join();
        return Err(error);
    }

    let started = Instant::now();
    let end = loop {
        if cancellation.is_some_and(ProcessCancellation::is_cancelled) {
            guard.terminate(1);
            let _ = child.wait();
            break ProcessEnd::Cancelled;
        }
        if started.elapsed() >= timeout {
            guard.terminate(1);
            let _ = child.wait();
            break ProcessEnd::TimedOut;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let raw = platform::raw_exit_code(&child, status)?;
                break ProcessEnd::Exited(raw);
            }
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(error) => {
                guard.terminate(1);
                let _ = child.wait();
                return Err(format!("프로세스 상태를 확인하지 못했습니다: {error}"));
            }
        }
    };

    if matches!(end, ProcessEnd::Exited(_)) {
        guard.terminate(1);
    }
    drop(guard);
    let stdout = stdout_drain
        .join()
        .map_err(|_| "프로세스 stdout 수집 스레드가 실패했습니다.".to_string())??;
    let stderr = stderr_drain
        .join()
        .map_err(|_| "프로세스 stderr 수집 스레드가 실패했습니다.".to_string())??;
    Ok(ProcessOutput {
        end,
        stdout,
        stderr,
    })
}

fn drain(mut pipe: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)
        .map_err(|error| format!("프로세스 출력 파이프를 읽지 못했습니다: {error}"))?;
    Ok(bytes)
}

#[cfg(windows)]
mod platform {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, ExitStatus};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED,
        THREAD_SUSPEND_RESUME,
    };

    pub(super) fn configure_suspended(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }

    pub(super) struct ProcessTreeGuard {
        job: HANDLE,
        primary_thread: HANDLE,
    }

    impl ProcessTreeGuard {
        pub(super) fn attach(child: &Child) -> Result<Self, String> {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(last_error("Windows Job Object를 만들지 못했습니다"));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                unsafe { CloseHandle(job) };
                return Err(last_error("Windows Job Object 제한을 설정하지 못했습니다"));
            }
            let assigned =
                unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
            if assigned == 0 {
                unsafe { CloseHandle(job) };
                return Err(last_error(
                    "프로세스를 Windows Job Object에 넣지 못했습니다",
                ));
            }
            let primary_thread = match suspended_thread(child.id()) {
                Ok(thread) => thread,
                Err(error) => {
                    unsafe {
                        TerminateJobObject(job, 1);
                        CloseHandle(job);
                    }
                    return Err(error);
                }
            };
            Ok(Self {
                job,
                primary_thread,
            })
        }

        pub(super) fn resume(&self) -> Result<(), String> {
            let previous = unsafe { ResumeThread(self.primary_thread) };
            if previous == u32::MAX {
                return Err(last_error("일시 중단된 프로세스를 재개하지 못했습니다"));
            }
            Ok(())
        }

        pub(super) fn terminate(&self, code: u32) {
            unsafe {
                TerminateJobObject(self.job, code);
            }
        }
    }

    impl Drop for ProcessTreeGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.primary_thread);
                CloseHandle(self.job);
            }
        }
    }

    pub(super) fn raw_exit_code(child: &Child, _status: ExitStatus) -> Result<u32, String> {
        let mut code = 0_u32;
        let ok = unsafe { GetExitCodeProcess(child.as_raw_handle() as HANDLE, &mut code) };
        if ok == 0 {
            return Err(last_error("프로세스 종료 코드를 읽지 못했습니다"));
        }
        Ok(code)
    }

    fn suspended_thread(process_id: u32) -> Result<HANDLE, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(last_error("프로세스 스레드 목록을 열지 못했습니다"));
        }
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut found = None;
        let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while has_entry {
            if entry.th32OwnerProcessID == process_id {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    found = Some(thread);
                    break;
                }
            }
            has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        found.ok_or_else(|| "일시 중단된 프로세스의 기본 스레드를 찾지 못했습니다.".to_string())
    }

    fn last_error(action: &str) -> String {
        format!("{action}: {}", std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command, ExitStatus};

    /// Unix has no Job Object; the child leads a fresh process group instead so
    /// timeout, cancellation, and root exit can signal every descendant that did
    /// not deliberately leave the group (`setsid`/`setpgid`).
    pub(super) fn configure_suspended(command: &mut Command) {
        command.process_group(0);
    }

    pub(super) struct ProcessTreeGuard {
        group: libc::pid_t,
    }

    impl ProcessTreeGuard {
        pub(super) fn attach(child: &Child) -> Result<Self, String> {
            let group = libc::pid_t::try_from(child.id())
                .map_err(|_| "프로세스 그룹 ID가 올바르지 않습니다.".to_string())?;
            Ok(Self { group })
        }

        pub(super) fn resume(&self) -> Result<(), String> {
            Ok(())
        }

        pub(super) fn terminate(&self, _code: u32) {
            // ESRCH (group already empty) is the expected success case after exit.
            unsafe { libc::killpg(self.group, libc::SIGKILL) };
        }
    }

    /// Mirror the Job Object's kill-on-close: an early return never leaks the group.
    impl Drop for ProcessTreeGuard {
        fn drop(&mut self) {
            self.terminate(1);
        }
    }

    /// Signal deaths map to the shell convention `128 + signal`.
    pub(super) fn raw_exit_code(_child: &Child, status: ExitStatus) -> Result<u32, String> {
        status
            .code()
            .map(|code| code as u32)
            .or_else(|| status.signal().map(|signal| 128 + signal as u32))
            .ok_or_else(|| "프로세스 종료 코드를 읽지 못했습니다.".to_string())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn command(script: &str) -> Command {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", script]);
        command
    }

    #[test]
    fn drains_large_stdout_and_stderr_without_deadlock() {
        let output = run_process_tree(
            command("for /L %i in (1,1,12000) do @echo stdout-%i & @echo stderr-%i 1>&2"),
            Duration::from_secs(20),
            None,
        )
        .unwrap();
        assert_eq!(output.end, ProcessEnd::Exited(0));
        assert!(output.stdout.len() > 64 * 1024);
        assert!(output.stderr.len() > 64 * 1024);
    }

    #[test]
    fn preserves_unsigned_ntstatus_exit_code() {
        let output =
            run_process_tree(command("exit /B -1073741819"), Duration::from_secs(5), None).unwrap();
        assert_eq!(output.end, ProcessEnd::Exited(0xC000_0005));
    }

    #[test]
    fn timeout_kills_grandchild_before_it_can_write() {
        let marker = std::env::temp_dir().join(format!(
            "eud-agent-process-tree-{}.txt",
            uuid::Uuid::new_v4().simple()
        ));
        let marker_text = marker.to_string_lossy().replace('"', "");
        let script = format!(
            "start \"\" /B cmd.exe /D /S /C \"ping -n 4 127.0.0.1 >nul & echo leaked>\\\"{marker_text}\\\"\" & ping -n 30 127.0.0.1 >nul"
        );
        let output = run_process_tree(command(&script), Duration::from_millis(250), None).unwrap();
        assert_eq!(output.end, ProcessEnd::TimedOut);
        thread::sleep(Duration::from_secs(4));
        assert!(!PathBuf::from(&marker).exists());
        fs::remove_file(marker).ok();
    }

    #[test]
    fn root_exit_terminates_pipe_inheriting_grandchild() {
        let marker = std::env::temp_dir().join(format!(
            "eud-agent-process-tree-root-exit-{}.txt",
            uuid::Uuid::new_v4().simple()
        ));
        let grandchild = marker.with_extension("ps1");
        fs::write(
            &grandchild,
            format!(
                "Start-Sleep -Seconds 4\nSet-Content -LiteralPath '{}' -Value leaked\n",
                marker.to_string_lossy().replace('\'', "''")
            ),
        )
        .unwrap();
        let script = format!(
            "Start-Process powershell.exe -ArgumentList '-NoProfile','-File','{}' -NoNewWindow",
            grandchild.to_string_lossy().replace('\'', "''")
        );
        let mut root = Command::new("powershell.exe");
        root.args(["-NoProfile", "-Command", &script]);
        let started = Instant::now();
        let output = run_process_tree(root, Duration::from_secs(10), None).unwrap();
        assert_eq!(output.end, ProcessEnd::Exited(0));
        assert!(started.elapsed() < Duration::from_secs(3));
        thread::sleep(Duration::from_secs(5));
        assert!(!marker.exists());
        fs::remove_file(marker).ok();
        fs::remove_file(grandchild).ok();
    }

    #[test]
    fn cancellation_terminates_process_tree() {
        let cancellation = ProcessCancellation::default();
        let trigger = cancellation.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            trigger.cancel();
        });
        let output = run_process_tree(
            command("ping -n 30 127.0.0.1 >nul"),
            Duration::from_secs(10),
            Some(&cancellation),
        )
        .unwrap();
        cancel_thread.join().unwrap();
        assert_eq!(output.end, ProcessEnd::Cancelled);
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::fs;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn marker(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eud-agent-process-tree-{tag}-{}.txt",
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn drains_large_stdout_and_stderr_without_deadlock() {
        let output = run_process_tree(
            sh("i=0; while [ $i -lt 12000 ]; do echo stdout-$i; echo stderr-$i 1>&2; i=$((i+1)); done"),
            Duration::from_secs(20),
            None,
        )
        .unwrap();
        assert_eq!(output.end, ProcessEnd::Exited(0));
        assert!(output.stdout.len() > 64 * 1024);
        assert!(output.stderr.len() > 64 * 1024);
    }

    #[test]
    fn signal_death_reports_shell_style_exit_code() {
        let output = run_process_tree(sh("kill -9 $$"), Duration::from_secs(5), None).unwrap();
        assert_eq!(output.end, ProcessEnd::Exited(128 + 9));
    }

    #[test]
    fn timeout_kills_grandchild_before_it_can_write() {
        let marker = marker("timeout");
        let script = format!("(sleep 2; echo leaked > '{}') & sleep 30", marker.display());
        let output = run_process_tree(sh(&script), Duration::from_millis(250), None).unwrap();
        assert_eq!(output.end, ProcessEnd::TimedOut);
        thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists());
        fs::remove_file(marker).ok();
    }

    #[test]
    fn root_exit_terminates_pipe_inheriting_grandchild() {
        let marker = marker("root-exit");
        let script = format!("(sleep 2; echo leaked > '{}') &", marker.display());
        let started = Instant::now();
        let output = run_process_tree(sh(&script), Duration::from_secs(10), None).unwrap();
        assert_eq!(output.end, ProcessEnd::Exited(0));
        assert!(started.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists());
        fs::remove_file(marker).ok();
    }

    #[test]
    fn cancellation_terminates_process_tree() {
        let cancellation = ProcessCancellation::default();
        let trigger = cancellation.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            trigger.cancel();
        });
        let output =
            run_process_tree(sh("sleep 30"), Duration::from_secs(10), Some(&cancellation)).unwrap();
        cancel_thread.join().unwrap();
        assert_eq!(output.end, ProcessEnd::Cancelled);
    }
}
