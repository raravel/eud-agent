//! Editor build orchestration and euddraft error capture.
//!
//! EUD Editor 3 generates the current `.eds` and build-side assets in memory, so
//! the backend must start the editor build first. The editor exposes macro/Lua
//! errors through `BUILDERR`, but sends ordinary euddraft stdout/stderr only to
//! its UI. When the editor build produces no fresh output map and no macro error,
//! this module re-runs `euddraft.exe <eds>` once and captures that missing output.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::bridge_io::{BridgeIo, SendOpts, HEARTBEAT_STALE_AFTER};

const BUILD_TIMEOUT: Duration = Duration::from_secs(300);
const BUILD_START_GRACE: Duration = Duration::from_millis(1_500);
const BUILD_POLL_INTERVAL: Duration = Duration::from_millis(200);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TRACEBACK_MARKER: &str = "Traceback (most recent call last):";

/// One build error returned directly by `build_run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildError {
    pub source: String,
    pub file: String,
    pub line: u64,
    pub message: String,
    pub raw: String,
}

/// Complete result of one editor build attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildRunResult {
    pub ok: bool,
    pub errors: Vec<BuildError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedProcess {
    success: bool,
    stdout: String,
    stderr: String,
}

trait BuildHost {
    fn edspath(&self) -> Result<String, String>;
    fn file_mtime(&self, path: &Path) -> Result<Option<SystemTime>, String>;
    fn start_editor_build(&self) -> Result<(), String>;
    fn wait_editor_build(&self) -> Result<(), String>;
    fn macro_errors(&self) -> Result<String, String>;
    fn euddraft_path(&self) -> Result<String, String>;
    fn run_euddraft(&self, executable: &Path, eds_path: &Path) -> Result<CapturedProcess, String>;
}

struct SystemBuildHost<'a> {
    bridge: &'a BridgeIo,
}

impl SystemBuildHost<'_> {
    fn build_opts() -> SendOpts {
        SendOpts {
            timeout: Duration::from_secs(10),
            busy_timeout: BUILD_TIMEOUT,
            poll_interval: BUILD_POLL_INTERVAL,
        }
    }
}

impl BuildHost for SystemBuildHost<'_> {
    fn edspath(&self) -> Result<String, String> {
        self.bridge
            .send("EDSPATH", &Self::build_opts(), None)
            .map_err(|error| error.to_string())
    }

    fn file_mtime(&self, path: &Path) -> Result<Option<SystemTime>, String> {
        match fs::metadata(path).and_then(|metadata| metadata.modified()) {
            Ok(modified) => Ok(Some(modified)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "failed to read file timestamp '{}': {error}",
                path.display()
            )),
        }
    }

    fn start_editor_build(&self) -> Result<(), String> {
        self.bridge
            .build(&Self::build_opts(), None)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn wait_editor_build(&self) -> Result<(), String> {
        let started = Instant::now();
        let mut observed_compiling = false;

        loop {
            let status = self
                .bridge
                .read_status_snapshot(HEARTBEAT_STALE_AFTER)
                .map_err(|error| format!("failed to read editor build status: {error}"))?;
            if status.compiling {
                observed_compiling = true;
            } else if observed_compiling || started.elapsed() >= BUILD_START_GRACE {
                return Ok(());
            }

            if started.elapsed() >= BUILD_TIMEOUT {
                return Err(format!(
                    "editor build did not finish within {}s",
                    BUILD_TIMEOUT.as_secs()
                ));
            }
            thread::sleep(BUILD_POLL_INTERVAL);
        }
    }

    fn macro_errors(&self) -> Result<String, String> {
        // The bridge skips inbox processing while the editor is compiling. This
        // call is therefore also a completion barrier if the status transition
        // happened between polls.
        self.bridge
            .send("BUILDERR", &Self::build_opts(), None)
            .map_err(|error| error.to_string())
    }

    fn euddraft_path(&self) -> Result<String, String> {
        self.bridge
            .send("GETSET program|euddraft", &Self::build_opts(), None)
            .map(|reply| parse_setting_value(&reply))
            .map_err(|error| error.to_string())
    }

    fn run_euddraft(&self, executable: &Path, eds_path: &Path) -> Result<CapturedProcess, String> {
        run_euddraft_process(executable, eds_path, BUILD_TIMEOUT)
    }
}

/// Run one authoritative editor build and return its errors in the same result.
pub fn build_run(bridge: &BridgeIo) -> Result<BuildRunResult, String> {
    run_build(&SystemBuildHost { bridge })
}

fn run_build(host: &impl BuildHost) -> Result<BuildRunResult, String> {
    let (eds_path, output_map) = parse_edspath(&host.edspath()?)?;
    let before_eds_mtime = host.file_mtime(&eds_path)?;
    let before_output_mtime = host.file_mtime(&output_map)?;

    host.start_editor_build()?;
    host.wait_editor_build()?;

    let macro_errors = parse_macro_errors(&host.macro_errors()?);
    if !macro_errors.is_empty() {
        return Ok(BuildRunResult {
            ok: false,
            errors: macro_errors,
        });
    }

    if is_fresh_output(before_output_mtime, host.file_mtime(&output_map)?) {
        return Ok(BuildRunResult {
            ok: true,
            errors: Vec::new(),
        });
    }

    if !is_fresh_output(before_eds_mtime, host.file_mtime(&eds_path)?) {
        return Ok(BuildRunResult {
            ok: false,
            errors: vec![BuildError {
                source: "editor".to_string(),
                file: String::new(),
                line: 0,
                message: "editor build failed before generating a fresh eds file".to_string(),
                raw: "The editor exposed no macro error and did not update the generated eds or output map; inspect the EUD Editor build log.".to_string(),
            }],
        });
    }

    let euddraft_path = PathBuf::from(host.euddraft_path()?);
    let captured = host.run_euddraft(&euddraft_path, &eds_path)?;
    if captured.success && is_fresh_output(before_output_mtime, host.file_mtime(&output_map)?) {
        return Ok(BuildRunResult {
            ok: true,
            errors: Vec::new(),
        });
    }

    let mut errors = parse_euddraft_output(&captured.stdout, &captured.stderr);
    if errors.is_empty() {
        let raw = joined_output(&captured.stdout, &captured.stderr);
        errors.push(BuildError {
            source: "euddraft".to_string(),
            file: String::new(),
            line: 0,
            message: if raw.is_empty() {
                "euddraft failed without producing diagnostic output".to_string()
            } else {
                "euddraft failed with an unrecognized diagnostic format".to_string()
            },
            raw,
        });
    }

    Ok(BuildRunResult { ok: false, errors })
}

fn is_fresh_output(before: Option<SystemTime>, after: Option<SystemTime>) -> bool {
    match (before, after) {
        (None, Some(_)) => true,
        (Some(before), Some(after)) => after > before,
        _ => false,
    }
}

fn run_euddraft_process(
    executable: &Path,
    eds_path: &Path,
    timeout: Duration,
) -> Result<CapturedProcess, String> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(format!(
            "euddraft executable is missing or not an absolute file: '{}'",
            executable.display()
        ));
    }
    if !eds_path.is_absolute() || !eds_path.is_file() {
        return Err(format!(
            "generated eds file is missing or not absolute: '{}'",
            eds_path.display()
        ));
    }
    let cwd = eds_path
        .parent()
        .ok_or_else(|| format!("generated eds path has no parent: '{}'", eds_path.display()))?;

    let mut command = Command::new(executable);
    command
        .arg(eds_path)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);

    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to start euddraft executable '{}': {error}",
            executable.display()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture euddraft stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture euddraft stderr".to_string())?;
    let stdout_reader = thread::spawn(move || read_stream(stdout, "stdout"));
    let stderr_reader = thread::spawn(move || read_stream(stderr, "stderr"));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(PROCESS_POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "euddraft did not finish within {}s and was terminated",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("failed while waiting for euddraft: {error}"));
            }
        }
    };

    let stdout = join_stream(stdout_reader, "stdout")?;
    let stderr = join_stream(stderr_reader, "stderr")?;
    Ok(CapturedProcess {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn read_stream(mut stream: impl Read, name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read euddraft {name}: {error}"))?;
    Ok(bytes)
}

fn join_stream(
    reader: thread::JoinHandle<Result<Vec<u8>, String>>,
    name: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("euddraft {name} reader panicked"))?
}

#[allow(unused_variables)]
fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
}

fn parse_setting_value(reply: &str) -> String {
    reply
        .split_once(" = ")
        .map_or(reply, |(_, value)| value)
        .trim()
        .to_string()
}

fn parse_edspath(reply: &str) -> Result<(PathBuf, PathBuf), String> {
    let mut lines = reply.lines().map(str::trim);
    let eds_path = lines.next().unwrap_or_default();
    let output_map = lines.next().unwrap_or_default();
    if eds_path.is_empty() || output_map.is_empty() {
        return Err(
            "EDSPATH did not return both the generated eds path and output map".to_string(),
        );
    }
    Ok((PathBuf::from(eds_path), PathBuf::from(output_map)))
}

fn parse_macro_errors(reply: &str) -> Vec<BuildError> {
    reply
        .lines()
        .filter_map(|line| {
            let raw = line.trim();
            if raw.is_empty() {
                return None;
            }
            parse_module_line(raw).or_else(|| {
                Some(BuildError {
                    source: "macro".to_string(),
                    file: String::new(),
                    line: 0,
                    message: raw.to_string(),
                    raw: raw.to_string(),
                })
            })
        })
        .map(|mut error| {
            error.source = "macro".to_string();
            error
        })
        .collect()
}

fn parse_euddraft_output(stdout: &str, stderr: &str) -> Vec<BuildError> {
    let blob = joined_output(stdout, stderr);
    let module_errors: Vec<BuildError> = blob.lines().filter_map(parse_module_line).collect();
    if !module_errors.is_empty() {
        return module_errors;
    }

    let description = last_traceback_description(&blob);
    let frame = first_traceback_frame(&blob);
    if description.is_none() && frame.is_none() {
        return Vec::new();
    }

    let (file, line) = frame.unwrap_or_else(|| (String::new(), 0));
    let message = description.unwrap_or_default();
    if file.is_empty() && message.is_empty() {
        return Vec::new();
    }
    vec![BuildError {
        source: "euddraft".to_string(),
        file,
        line,
        message,
        raw: blob.trim().to_string(),
    }]
}

fn parse_module_line(line: &str) -> Option<BuildError> {
    let error_start = line.find("[Error")?;
    let line = &line[error_start..];
    let module_marker = "] Module \"";
    let module_start = line.find(module_marker)? + module_marker.len();
    let module_tail = &line[module_start..];
    let module_end = module_tail.find('"')?;
    let file = module_tail[..module_end].trim();
    let location = module_tail[module_end + 1..].strip_prefix(" Line ")?;
    let (line_number, message) = location.split_once(" : ")?;
    Some(BuildError {
        source: "euddraft".to_string(),
        file: file.to_string(),
        line: line_number.trim().parse().ok()?,
        message: message.trim().to_string(),
        raw: line.trim().to_string(),
    })
}

fn last_traceback_description(blob: &str) -> Option<String> {
    let mut search_from = 0;
    let mut last = None;
    while let Some(relative) = blob[search_from..].find(TRACEBACK_MARKER) {
        let marker = search_from + relative;
        if let Some(error_start) = blob[..marker].rfind("[Error]") {
            last = Some(
                blob[error_start + "[Error]".len()..marker]
                    .trim()
                    .to_string(),
            );
        }
        search_from = marker + TRACEBACK_MARKER.len();
    }
    last
}

fn first_traceback_frame(blob: &str) -> Option<(String, u64)> {
    for line in blob.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("File \"") else {
            continue;
        };
        let Some((path, location)) = rest.split_once("\", line ") else {
            continue;
        };
        let Some((line_number, function)) = location.split_once(", in ") else {
            continue;
        };
        if function.is_empty()
            || !function
                .chars()
                .all(|character| character == '_' || character.is_alphanumeric())
        {
            continue;
        }
        let file = path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(path)
            .split('.')
            .next()
            .unwrap_or_default()
            .to_string();
        if let Ok(line_number) = line_number.trim().parse() {
            return Some((file, line_number));
        }
    }
    None
}

fn joined_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        (stdout, "") => stdout.to_string(),
        ("", stderr) => stderr.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::collections::VecDeque;

    use super::*;

    #[test]
    fn parses_module_line_errors_from_euddraft_output() {
        let stderr = r#"[Error] Module "main" Line 12 : Undefined identifier: foo"#;

        let errors = parse_euddraft_output("", stderr);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, "euddraft");
        assert_eq!(errors[0].file, "main");
        assert_eq!(errors[0].line, 12);
        assert_eq!(errors[0].message, "Undefined identifier: foo");
    }

    #[test]
    fn parses_multiline_traceback_description_and_first_frame() {
        let stderr = r#"[Error] 연결맵에 조건에 맞는 플레이어가 없습니다.
Human 컨트롤러와 시작 위치가 필요합니다.
Traceback (most recent call last):
  File "C:\project\main.eps", line 27, in onPluginStart
  File "C:\eudplib\core.py", line 8, in compile
RuntimeError: failed"#;

        let errors = parse_euddraft_output("", stderr);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, "euddraft");
        assert_eq!(errors[0].file, "main");
        assert_eq!(errors[0].line, 27);
        assert_eq!(
            errors[0].message,
            "연결맵에 조건에 맞는 플레이어가 없습니다.\nHuman 컨트롤러와 시작 위치가 필요합니다."
        );
    }

    #[test]
    fn parses_edspath_and_macro_errors() {
        let (eds, output) = parse_edspath("C:\\temp\\EUDEditor.eds\r\nC:\\maps\\out.scx\r\n")
            .expect("two EDSPATH lines");
        assert_eq!(eds, PathBuf::from(r"C:\temp\EUDEditor.eds"));
        assert_eq!(output, PathBuf::from(r"C:\maps\out.scx"));

        let errors = parse_macro_errors("first macro error\r\nsecond macro error\r\n");
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].source, "macro");
        assert_eq!(errors[0].message, "first macro error");
        assert_eq!(errors[1].message, "second macro error");
    }

    #[test]
    fn fresh_editor_output_succeeds_without_direct_rerun() {
        let before = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let after = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let host = FakeHost::new(
            vec![Some(before), Some(before), Some(after)],
            "",
            failed_process(),
        );

        let result = run_build(&host).expect("build result");

        assert!(result.ok);
        assert!(result.errors.is_empty());
        assert!(!host.calls().contains(&"run_euddraft"));
    }

    #[test]
    fn macro_errors_short_circuit_direct_rerun() {
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let host = FakeHost::new(
            vec![Some(timestamp), Some(timestamp)],
            "macro exploded",
            failed_process(),
        );

        let result = run_build(&host).expect("build result");

        assert!(!result.ok);
        assert_eq!(result.errors[0].source, "macro");
        assert!(!host.calls().contains(&"run_euddraft"));
    }

    #[test]
    fn failed_editor_build_reruns_fresh_eds_and_returns_structured_errors() {
        let before = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let after = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let host = FakeHost::new(
            vec![Some(before), Some(before), Some(before), Some(after)],
            "",
            CapturedProcess {
                success: false,
                stdout: String::new(),
                stderr: r#"[Error] Module "main" Line 7 : Expected ;"#.to_string(),
            },
        );

        let result = run_build(&host).expect("build result");

        assert!(!result.ok);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].file, "main");
        assert_eq!(result.errors[0].line, 7);
        assert_eq!(result.errors[0].message, "Expected ;");
        assert!(host.calls().contains(&"run_euddraft"));
    }

    #[test]
    fn unrecognized_euddraft_failure_preserves_raw_output() {
        let before = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let after = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let host = FakeHost::new(
            vec![Some(before), Some(before), Some(before), Some(after)],
            "",
            failed_process(),
        );

        let result = run_build(&host).expect("build result");

        assert!(!result.ok);
        assert_eq!(
            result.errors[0].message,
            "euddraft failed with an unrecognized diagnostic format"
        );
        assert_eq!(result.errors[0].raw, "unrecognized failure");
    }

    #[test]
    fn stale_eds_is_never_rerun_after_editor_generation_failure() {
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let host = FakeHost::new(
            vec![
                Some(timestamp),
                Some(timestamp),
                Some(timestamp),
                Some(timestamp),
            ],
            "",
            failed_process(),
        );

        let result = run_build(&host).expect("build result");

        assert!(!result.ok);
        assert_eq!(result.errors[0].source, "editor");
        assert!(!host.calls().contains(&"run_euddraft"));
    }

    fn failed_process() -> CapturedProcess {
        CapturedProcess {
            success: false,
            stdout: String::new(),
            stderr: "unrecognized failure".to_string(),
        }
    }

    struct FakeHost {
        mtimes: Mutex<VecDeque<Option<SystemTime>>>,
        macro_errors: String,
        process: CapturedProcess,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeHost {
        fn new(
            mtimes: Vec<Option<SystemTime>>,
            macro_errors: impl Into<String>,
            process: CapturedProcess,
        ) -> Self {
            Self {
                mtimes: Mutex::new(mtimes.into()),
                macro_errors: macro_errors.into(),
                process,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().clone()
        }

        fn record(&self, call: &'static str) {
            self.calls.lock().push(call);
        }
    }

    impl BuildHost for FakeHost {
        fn edspath(&self) -> Result<String, String> {
            self.record("edspath");
            Ok("C:\\temp\\EUDEditor.eds\r\nC:\\maps\\out.scx".to_string())
        }

        fn file_mtime(&self, _path: &Path) -> Result<Option<SystemTime>, String> {
            self.record("file_mtime");
            self.mtimes
                .lock()
                .pop_front()
                .ok_or_else(|| "missing fake mtime".to_string())
        }

        fn start_editor_build(&self) -> Result<(), String> {
            self.record("start_editor_build");
            Ok(())
        }

        fn wait_editor_build(&self) -> Result<(), String> {
            self.record("wait_editor_build");
            Ok(())
        }

        fn macro_errors(&self) -> Result<String, String> {
            self.record("macro_errors");
            Ok(self.macro_errors.clone())
        }

        fn euddraft_path(&self) -> Result<String, String> {
            self.record("euddraft_path");
            Ok(r"C:\tools\euddraft.exe".to_string())
        }

        fn run_euddraft(
            &self,
            _executable: &Path,
            _eds_path: &Path,
        ) -> Result<CapturedProcess, String> {
            self.record("run_euddraft");
            Ok(self.process.clone())
        }
    }
    #[test]
    #[ignore = "requires the configured live EUD Editor project and bridge"]
    fn live_complete_project_build_run() {
        let dirs = crate::config::DataDirs::from_bases(
            Path::new(&std::env::var("APPDATA").unwrap()),
            Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
        );
        let bridge = crate::ipc::bridge_from_config(&dirs).unwrap();
        let result = build_run(&bridge).unwrap();
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        assert!(result.ok, "live complete build failed: {:?}", result.errors);
    }
}
