//! Low-level codex subprocess client and prompt composer.
//!
//! This module mirrors `server/eud_agent/codex_client.py`: it composes the small
//! codex-facing prompt shape, invokes the user's resolved `codex exec` shim, and extracts
//! epScript code from codex stdout.
//!
//! The fenced-output contract is intentional. `generate()` extracts fenced code blocks and
//! returns `CodexError::NoCode` when there are none. Per rules.md "codex invocation", codex
//! stdout is treated as noisy, so the client fails with the raw output snippet rather than
//! applying unfenced banner or usage text to the editor. The `codex exec` CLI wraps model
//! output in fenced blocks, so fenced output is the normal success path; `NoCode` indicates
//! a real failure such as banner-only output or an argument/usage error. The prompt's
//! "코드만" instruction removes explanatory prose, not the code fence.
//!
//! Scope and layering are deliberately narrow here. This composer emits only the
//! low-level `참고자료` / `현재 코드` / `요청` / `epScript 코드` framing. The
//! first-principles, evidence, and message-format system guardrails are assembled upstream
//! by the engine/orchestrator, as described in feature 11's engine section. That matches
//! the Python layering where `engine.py` wraps `codex_client.build_prompt`. Callers wiring
//! generation must go through the engine so those guardrails apply.

use std::{
    env,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use thiserror::Error;
use tokio::{io::AsyncWriteExt, process::Command, time};

use crate::ipc;

const SYSTEM_PROMPT: &str =
    "너는 스타크래프트 EUD 맵 제작용 epScript(eps) 코드를 작성하는 어시스턴트다. \
아래 [참고자료]는 네이버 카페/공식 매뉴얼에서 검색한 eps/eud3 지식이다. \
사용자 요청을 만족하는 epScript 코드만 출력해라. 설명/마크다운 없이 코드만. \
플레이어 루프·변수 선언 등 eps 관례를 지켜라.";

const RAW_SNIPPET_LIMIT: usize = 500;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
const APP_SERVER_STDERR_TAIL_LIMIT: usize = 4 * 1024;
const APP_SERVER_EXIT_DIAGNOSTIC_WAIT: Duration = Duration::from_millis(50);
pub(crate) const LARGE_CONTEXT_WINDOW_TOKENS: i64 = 1_000_000;
const LARGE_CONTEXT_AUTO_COMPACT_TOKENS: i64 = 900_000;

/// Windows `CREATE_NO_WINDOW` process-creation flag. The GUI app is windowless,
/// so spawning the `codex.cmd` batch shim would otherwise flash a console window
/// for every turn; applied at each codex spawn to keep the agent headless.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodexError {
    #[error("codex command not found: {0}")]
    NotFound(String),
    #[error("codex exec timed out: {0}")]
    Timeout(String),
    #[error("codex produced no fenced code block: {0}")]
    NoCode(String),
}

/// Resolve the Codex command path.
///
/// Honors a `CODEX_CMD` env override (full path to the shim); otherwise prefers the
/// app-managed distribution and finally locates `codex` on PATH via the `which` crate.
/// The app-managed distribution is usable only when both fixed-name runtime helpers exist.
/// This never returns a bare `"codex"` command.
pub fn resolve_codex_cmd() -> Result<PathBuf, CodexError> {
    if let Some(cmd) = env::var_os("CODEX_CMD").filter(|cmd| !cmd.is_empty()) {
        return Ok(PathBuf::from(cmd));
    }

    // The app-installed distribution is resolved BEFORE PATH so a fresh setup install is
    // found without a restart. A CLI without either fixed-name runtime helper can start but
    // fails Code Mode execution or elevated sandbox initialization, so keep that partial
    // install behind the setup gate.
    let mut missing_app_component = None;
    if let Some(local) = env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        match app_managed_codex(Path::new(&local)) {
            Ok(Some(candidate)) => return Ok(candidate),
            Ok(None) => {}
            Err(missing_component) => missing_app_component = Some(missing_component),
        }
    }

    which::which("codex").map_err(|err| {
        let detail = match missing_app_component {
            Some(component) => format!(
                "app-installed Codex is incomplete: missing {}. Reinstall Codex from the setup screen",
                component.display()
            ),
            None => format!(
                "could not resolve codex via CODEX_CMD, the app bin dir, or PATH: {err}. Install codex (the setup screen can do this) or set CODEX_CMD to the codex binary path"
            ),
        };
        CodexError::NotFound(detail)
    })
}

/// The app-installed Codex binary path under `<local_app_data>\eud-agent\bin`
/// (matches [`DataDirs::bin_dir`](crate::config::DataDirs::bin_dir)).
fn well_known_codex_path(local_app_data: &Path) -> PathBuf {
    local_app_data
        .join("eud-agent")
        .join("bin")
        .join(crate::bootstrap::CODEX_BIN_FILENAME)
}

fn well_known_codex_host_path(local_app_data: &Path) -> PathBuf {
    local_app_data
        .join("eud-agent")
        .join("bin")
        .join(crate::bootstrap::CODEX_CODE_MODE_HOST_FILENAME)
}

fn well_known_codex_sandbox_setup_path(local_app_data: &Path) -> PathBuf {
    local_app_data
        .join("eud-agent")
        .join("bin")
        .join(crate::bootstrap::CODEX_SANDBOX_SETUP_FILENAME)
}

/// `Ok(None)` means no app-managed CLI; `Err(path)` means the CLI exists but a required
/// fixed-name runtime helper is missing.
fn app_managed_codex(local_app_data: &Path) -> Result<Option<PathBuf>, PathBuf> {
    let codex = well_known_codex_path(local_app_data);
    if !codex.is_file() {
        return Ok(None);
    }
    let host = well_known_codex_host_path(local_app_data);
    if !host.is_file() {
        return Err(host);
    }
    let sandbox_setup = well_known_codex_sandbox_setup_path(local_app_data);
    if !sandbox_setup.is_file() {
        return Err(sandbox_setup);
    }
    Ok(Some(codex))
}

pub fn extract_code(text: &str) -> Result<String, CodexError> {
    let raw = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut in_block = false;

    for line in raw.split('\n') {
        if in_block {
            if line.trim() == "```" {
                let block = current.join("\n").trim().to_string();
                if !block.is_empty() {
                    blocks.push(block);
                }
                current.clear();
                in_block = false;
            } else {
                current.push(line);
            }
        } else if line.trim_start().starts_with("```") {
            current.clear();
            in_block = true;
        }
    }

    if blocks.is_empty() {
        return Err(CodexError::NoCode(
            raw.chars().take(RAW_SNIPPET_LIMIT).collect(),
        ));
    }

    Ok(blocks.join("\n\n"))
}

/// Low-level prompt composer; safety and first-principles sections are added by the engine.
pub fn build_prompt(
    instruction: &str,
    context_chunks: &[String],
    current_code: Option<&str>,
) -> String {
    let chunks = context_chunks
        .iter()
        .filter(|chunk| !chunk.trim().is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>();
    let context = if chunks.is_empty() {
        "(없음)".to_string()
    } else {
        chunks.join("\n\n")
    };

    let mut parts = vec![
        SYSTEM_PROMPT.to_string(),
        String::new(),
        "[참고자료]".to_string(),
        context,
    ];

    if let Some(code) = current_code.filter(|code| !code.trim().is_empty()) {
        parts.extend([String::new(), "[현재 코드]".to_string(), code.to_string()]);
    }

    parts.extend([
        String::new(),
        "[요청]".to_string(),
        instruction.to_string(),
        String::new(),
        "[epScript 코드]".to_string(),
    ]);

    parts.join("\n")
}

#[derive(Debug, Clone)]
pub struct CodexClient {
    codex_cmd: PathBuf,
    repo_root: PathBuf,
}

impl CodexClient {
    pub fn new(
        codex_cmd: impl Into<PathBuf>,
        repo_root: impl Into<PathBuf>,
    ) -> Result<Self, CodexError> {
        let codex_cmd = codex_cmd.into();
        if codex_cmd.as_os_str().is_empty() {
            return Err(CodexError::NotFound(
                "codex path is empty; resolve codex.cmd before constructing CodexClient"
                    .to_string(),
            ));
        }
        if !codex_cmd.is_file() {
            return Err(CodexError::NotFound(format!(
                "codex path does not exist: {}",
                codex_cmd.display()
            )));
        }

        Ok(Self {
            codex_cmd,
            repo_root: repo_root.into(),
        })
    }

    pub async fn generate(&self, prompt: &str) -> Result<String, CodexError> {
        let mut command = Command::new(&self.codex_cmd);
        command
            .arg("exec")
            .arg("--skip-git-repo-check")
            .current_dir(&self.repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let mut child = command
            .spawn()
            .map_err(|err| CodexError::NotFound(format!("failed to spawn codex: {err}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            match stdin.write_all(prompt.as_bytes()).await {
                Ok(()) => {
                    if let Err(err) = stdin.shutdown().await {
                        if !matches!(
                            err.kind(),
                            ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                        ) {
                            return Err(CodexError::NoCode(format!(
                                "failed to close codex stdin: {err}"
                            )));
                        }
                    }
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                    ) => {}
                Err(err) => {
                    return Err(CodexError::NoCode(format!(
                        "failed to write codex stdin: {err}"
                    )));
                }
            }
        }

        let output = match time::timeout(DEFAULT_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return Err(CodexError::NoCode(format!(
                    "failed to read codex output: {err}"
                )));
            }
            Err(_) => {
                return Err(CodexError::Timeout(format!(
                    "codex exec timed out after {}s",
                    DEFAULT_TIMEOUT.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        match extract_code(&stdout) {
            Ok(code) => Ok(code),
            Err(CodexError::NoCode(snippet)) => {
                let tail = take_last_chars(stderr.trim(), RAW_SNIPPET_LIMIT);
                if tail.is_empty() {
                    Err(CodexError::NoCode(snippet))
                } else {
                    Err(CodexError::NoCode(format!(
                        "{snippet}\n--- stderr (tail) ---\n{tail}"
                    )))
                }
            }
            Err(err) => Err(err),
        }
    }
}

fn take_last_chars(text: &str, limit: usize) -> String {
    let mut chars = text.chars().rev().take(limit).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_single_fence_without_language_tag() {
        let output = "banner\n```\nfunction main() {\n    DoActions();\n}\n```\nusage";

        assert_eq!(
            extract_code(output).unwrap(),
            "function main() {\n    DoActions();\n}"
        );
    }

    #[test]
    fn extract_code_single_fence_with_language_tag() {
        let output = "```eps\nconst cp = getcurpl();\nsetcurpl(cp);\n```";

        assert_eq!(
            extract_code(output).unwrap(),
            "const cp = getcurpl();\nsetcurpl(cp);"
        );
    }

    #[test]
    fn extract_code_joins_multiple_blocks_with_blank_line() {
        let output = "```eps\nconst a = 1;\n```\nnoise\n```javascript\nconst b = 2;\n```";

        assert_eq!(
            extract_code(output).unwrap(),
            "const a = 1;\n\nconst b = 2;"
        );
    }

    #[test]
    fn extract_code_normalizes_crlf_to_lf() {
        let output = "```eps\r\nconst a = 1;\r\nconst b = 2;\r\n```";

        assert_eq!(extract_code(output).unwrap(), "const a = 1;\nconst b = 2;");
    }

    #[test]
    fn extract_code_requires_closing_fence_at_line_start() {
        let output =
            "```eps\nconst marker = \"inline ``` is not a close\";\nconst done = true;\n```";

        assert_eq!(
            extract_code(output).unwrap(),
            "const marker = \"inline ``` is not a close\";\nconst done = true;"
        );
    }

    #[test]
    fn extract_code_zero_fences_returns_no_code_with_truncated_raw_output() {
        let raw = format!("prefix {}", "x".repeat(700));
        let err = extract_code(&raw).unwrap_err();

        match err {
            CodexError::NoCode(snippet) => {
                assert!(snippet.contains("prefix "));
                assert_eq!(snippet.len(), 500);
                assert!(!snippet.contains(&"x".repeat(600)));
            }
            other => panic!("expected NoCode, got {other:?}"),
        }
    }

    #[test]
    fn build_prompt_empty_context_marks_none_and_includes_request_and_code_section() {
        let prompt = build_prompt("마린 생성", &[], None);

        assert!(prompt.contains("[참고자료]\n(없음)"));
        assert!(prompt.contains("[요청]\n마린 생성"));
        assert!(prompt.contains("[epScript 코드]"));
        assert!(!prompt.contains("[현재 코드]"));
    }

    #[test]
    fn build_prompt_includes_current_code_section_when_supplied() {
        let context = vec!["source: docs\nUse epScript.".to_string()];
        let prompt = build_prompt("수정", &context, Some("function before() {}"));

        assert!(prompt.contains("[참고자료]\nsource: docs\nUse epScript."));
        assert!(prompt.contains("[현재 코드]\nfunction before() {}"));
        assert!(prompt.contains("[요청]\n수정"));
        assert!(prompt.ends_with("[epScript 코드]"));
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccess {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexTurnInput {
    pub text: String,
    pub image_paths: Vec<PathBuf>,
    /// Session-owned per-project Codex cwd. `None` retains the hermetic
    /// read-only mode used by isolated protocol tests and non-project operations.
    pub workspace_root: Option<PathBuf>,
    pub workspace_access: WorkspaceAccess,
    /// Optional JSON Schema constraining the final assistant message.
    pub output_schema: Option<serde_json::Value>,
    /// Fail the turn if Codex attempts any tool call.
    pub forbid_tools: bool,
}

impl CodexTurnInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image_paths: Vec::new(),
            workspace_root: None,
            workspace_access: WorkspaceAccess::Read,
            output_schema: None,
            forbid_tools: false,
        }
    }

    pub fn with_access(mut self, access: WorkspaceAccess) -> Self {
        self.workspace_access = access;
        self
    }

    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn without_tools(mut self) -> Self {
        self.forbid_tools = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppServerEvent {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted,
    ReasoningDelta(String),
    AnswerDelta(String),
    ItemStarted {
        item_id: Option<String>,
    },
    ItemCompleted {
        item_id: Option<String>,
    },
    ContextCompactionStarted,
    ContextCompactionCompleted,
    /// A tool-like thread item opened (mcpToolCall / commandExecution /
    /// webSearch) — carries the tool name + argument text so the panel can
    /// render a live Tool card (EUD-068 classification, ported from v1).
    ToolCallStarted {
        name: String,
        args: Option<String>,
    },
    /// The matching tool-like thread item completed — result text + status
    /// ("completed" vs failed/declined) for the Tool card flip.
    ToolCallCompleted {
        name: String,
        result: Option<String>,
        status: Option<String>,
    },
    TokenUsageUpdated {
        turn_id: String,
        token_usage: ipc::ContextUsage,
    },
    TurnComplete,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AppServerError {
    pub message: String,
}

impl AppServerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AppServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppServerError {}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexReasoningEffortOption {
    pub reasoning_effort: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexModel {
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub supported_reasoning_efforts: Vec<CodexReasoningEffortOption>,
    pub default_reasoning_effort: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexModelSettings {
    pub models: Vec<CodexModel>,
    pub selected_model: String,
    pub selected_reasoning_effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexModelSelection {
    pub model: String,
    pub reasoning_effort: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelListPage {
    data: Vec<CodexModel>,
    next_cursor: Option<String>,
}

type AppServerRequestResult = Result<serde_json::Value, AppServerError>;
type AppServerPending = std::sync::Arc<
    tokio::sync::Mutex<
        std::collections::HashMap<u64, tokio::sync::oneshot::Sender<AppServerRequestResult>>,
    >,
>;
type AppServerWriter<W> = std::sync::Arc<tokio::sync::Mutex<W>>;

struct AppServerReadContext<W> {
    writer: AppServerWriter<W>,
    pending: AppServerPending,
    events_tx: tokio::sync::mpsc::Sender<AppServerEvent>,
    thread_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    thread_started: std::sync::Arc<tokio::sync::Notify>,
    turn_completed: tokio::sync::broadcast::Sender<()>,
    transport: std::sync::Arc<AppServerTransportState>,
    ask_runtime: Option<crate::tool_exec::SessionToolRuntime>,
}
#[derive(Default)]
struct AppServerTransportState {
    closed: std::sync::atomic::AtomicBool,
    exit_detail: std::sync::Mutex<Option<String>>,
    stderr_tail: std::sync::Mutex<Vec<u8>>,
    exit_observed: tokio::sync::Notify,
}

impl AppServerTransportState {
    fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Acquire)
    }

    fn mark_closed(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn record_exit(&self, result: std::io::Result<std::process::ExitStatus>) {
        let detail = match result {
            Ok(status) => format!("app-server exited with {status}"),
            Err(error) => format!("failed waiting for app-server exit: {error}"),
        };
        *self
            .exit_detail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(detail);
        self.mark_closed();
        self.exit_observed.notify_waiters();
    }

    fn append_stderr(&self, bytes: &[u8]) {
        let mut tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if bytes.len() >= APP_SERVER_STDERR_TAIL_LIMIT {
            tail.clear();
            tail.extend_from_slice(&bytes[bytes.len() - APP_SERVER_STDERR_TAIL_LIMIT..]);
            return;
        }
        let overflow = tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(APP_SERVER_STDERR_TAIL_LIMIT);
        if overflow > 0 {
            tail.drain(..overflow);
        }
        tail.extend_from_slice(bytes);
    }

    async fn wait_for_exit_detail(&self) {
        if self
            .exit_detail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            return;
        }
        let _ = time::timeout(
            APP_SERVER_EXIT_DIAGNOSTIC_WAIT,
            self.exit_observed.notified(),
        )
        .await;
    }

    fn failure_message(&self, reason: &str) -> String {
        let exit_detail = self
            .exit_detail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let stderr_tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let stderr_tail = String::from_utf8_lossy(&stderr_tail);
        let stderr_tail = stderr_tail.trim();

        let mut message = reason.to_string();
        if let Some(detail) = exit_detail {
            if detail != reason {
                message.push_str("; ");
                message.push_str(&detail);
            }
        }
        if !stderr_tail.is_empty() {
            message.push_str("; stderr: ");
            message.push_str(stderr_tail);
        }
        message.push_str("; app-server will restart automatically on the next request");
        message
    }
}

struct AppServerProcess {
    waiter: tokio::task::JoinHandle<()>,
    stderr_reader: tokio::task::JoinHandle<()>,
}

impl Drop for AppServerProcess {
    fn drop(&mut self) {
        self.waiter.abort();
        self.stderr_reader.abort();
    }
}

pub struct CodexAppServerClient<R, W> {
    _reader: std::marker::PhantomData<R>,
    writer: AppServerWriter<W>,
    pending: AppServerPending,
    next_id: u64,
    initialized: bool,
    model_selection: Option<CodexModelSelection>,
    large_context_enabled: bool,
    mcp_server_url: Option<String>,
    thread_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    thread_started: std::sync::Arc<tokio::sync::Notify>,
    turn_completed: tokio::sync::broadcast::Sender<()>,
    transport: std::sync::Arc<AppServerTransportState>,
    _process: Option<AppServerProcess>,
}

impl<R, W> CodexAppServerClient<R, W>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    pub fn new_with_stdio(
        reader: R,
        writer: W,
    ) -> (Self, tokio::sync::mpsc::Receiver<AppServerEvent>) {
        Self::new_with_stdio_and_ask_runtime(reader, writer, None)
    }

    fn new_with_stdio_and_ask_runtime(
        reader: R,
        writer: W,
        ask_runtime: Option<crate::tool_exec::SessionToolRuntime>,
    ) -> (Self, tokio::sync::mpsc::Receiver<AppServerEvent>) {
        let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
        let pending =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let thread_id = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let thread_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let transport = std::sync::Arc::new(AppServerTransportState::default());
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(128);
        let (turn_completed, _) = tokio::sync::broadcast::channel(16);

        tokio::spawn(read_app_server_stdout(
            reader,
            AppServerReadContext {
                writer: std::sync::Arc::clone(&writer),
                pending: std::sync::Arc::clone(&pending),
                events_tx,
                thread_id: std::sync::Arc::clone(&thread_id),
                thread_started: std::sync::Arc::clone(&thread_started),
                turn_completed: turn_completed.clone(),
                transport: std::sync::Arc::clone(&transport),
                ask_runtime,
            },
        ));

        (
            Self {
                _reader: std::marker::PhantomData,
                writer,
                pending,
                next_id: 1,
                initialized: false,
                model_selection: None,
                large_context_enabled: false,
                mcp_server_url: None,
                thread_id,
                thread_started,
                turn_completed,
                transport,
                _process: None,
            },
            events_rx,
        )
    }

    pub(crate) fn is_transport_closed(&self) -> bool {
        self.transport.is_closed()
    }

    async fn write_message(&self, value: serde_json::Value) -> Result<(), AppServerError> {
        if let Err(error) = write_json_rpc_line(&self.writer, value).await {
            self.transport.mark_closed();
            return Err(AppServerError::new(
                self.transport.failure_message(&error.message),
            ));
        }
        Ok(())
    }

    async fn ensure_initialized(&mut self) -> Result<(), AppServerError> {
        if self.initialized {
            return Ok(());
        }

        self.send_request(
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "eud-agent",
                    "title": null,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": null,
            }),
        )
        .await?;
        // Complete the app-server handshake before issuing model/thread
        // requests. The notification intentionally has no `id` or `params`.

        self.write_message(serde_json::json!({ "method": "initialized" }))
            .await?;
        self.initialized = true;
        Ok(())
    }
    /// Ensure the exact-root Windows sandbox backend is installed before a
    /// project thread can run. The custom profile requires the elevated backend;
    /// unsupported/denied setup fails closed instead of falling back to
    /// full-filesystem-read legacy workspace-write.
    #[cfg(windows)]
    pub async fn ensure_workspace_sandbox(&mut self, cwd: &Path) -> Result<(), AppServerError> {
        self.ensure_initialized().await?;
        if self.windows_sandbox_ready().await? {
            return Ok(());
        }

        self.send_request(
            "windowsSandbox/setupStart",
            serde_json::json!({
                "mode": "elevated",
                "cwd": path_text(cwd)?,
            }),
        )
        .await?;

        let deadline = time::Instant::now() + Duration::from_secs(120);
        while time::Instant::now() < deadline {
            time::sleep(Duration::from_millis(500)).await;
            if self.windows_sandbox_ready().await? {
                return Ok(());
            }
        }
        Err(AppServerError::new(
            "strict Windows workspace sandbox setup did not complete; approve the elevation prompt and retry",
        ))
    }

    #[cfg(not(windows))]
    pub async fn ensure_workspace_sandbox(&mut self, _cwd: &Path) -> Result<(), AppServerError> {
        Ok(())
    }

    #[cfg(windows)]
    async fn windows_sandbox_ready(&mut self) -> Result<bool, AppServerError> {
        let value = self
            .send_request("windowsSandbox/readiness", serde_json::Value::Null)
            .await?;
        Ok(value.get("status").and_then(serde_json::Value::as_str) == Some("ready"))
    }

    pub async fn list_models(&mut self) -> Result<Vec<CodexModel>, AppServerError> {
        self.ensure_initialized().await?;

        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let result = self
                .send_request(
                    "model/list",
                    serde_json::json!({
                        "cursor": cursor,
                        "limit": 100,
                        "includeHidden": false,
                    }),
                )
                .await?;
            let page: CodexModelListPage = serde_json::from_value(result).map_err(|err| {
                AppServerError::new(format!("invalid model/list response: {err}"))
            })?;
            models.extend(page.data);

            match page.next_cursor {
                Some(next) if cursor.as_deref() != Some(next.as_str()) => cursor = Some(next),
                Some(_) => {
                    return Err(AppServerError::new(
                        "model/list returned a repeated pagination cursor",
                    ));
                }
                None => break,
            }
        }
        Ok(models)
    }

    pub(crate) fn set_model_selection(&mut self, selection: Option<CodexModelSelection>) {
        self.model_selection = selection;
    }
    pub(crate) fn set_large_context_enabled(&mut self, enabled: bool) {
        self.large_context_enabled = enabled;
    }

    pub async fn start_compaction(&mut self) -> Result<(), AppServerError> {
        self.ensure_initialized().await?;
        let thread_id = self
            .current_thread_id()
            .await
            .ok_or_else(|| AppServerError::new("압축할 Codex 대화가 없습니다."))?;
        self.send_request(
            "thread/compact/start",
            serde_json::json!({ "threadId": thread_id }),
        )
        .await?;
        Ok(())
    }

    pub async fn run_turn(&mut self, input: CodexTurnInput) -> Result<(), AppServerError> {
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
        let interrupted = self.run_turn_cancellable(input, cancel_rx, 0).await?;
        debug_assert!(!interrupted);
        Ok(())
    }

    /// Run one turn and interrupt it when `cancellation` advances past the
    /// generation captured by the caller. The app-server requires both ids for
    /// `turn/interrupt`; `turn/start` returns the authoritative turn id.
    ///
    /// Returns `true` only after the interrupted turn emits `turn/completed`, so
    /// callers can safely unlock their UI and start the next turn.
    pub async fn run_turn_cancellable(
        &mut self,
        input: CodexTurnInput,
        mut cancellation: tokio::sync::watch::Receiver<u64>,
        cancellation_generation: u64,
    ) -> Result<bool, AppServerError> {
        self.ensure_initialized().await?;

        let mut turn_completed = self.turn_completed.subscribe();

        let thread_id = match self.current_thread_id().await {
            Some(thread_id) => {
                self.send_request(
                    "thread/resume",
                    thread_resume_params(
                        &thread_id,
                        input.workspace_root.as_deref(),
                        self.mcp_server_url.as_deref(),
                        self.large_context_enabled,
                    )?,
                )
                .await?;
                thread_id
            }
            None => {
                self.send_request(
                    "thread/start",
                    thread_start_params(
                        input.workspace_root.as_deref(),
                        self.mcp_server_url.as_deref(),
                        self.large_context_enabled,
                    )?,
                )
                .await?;
                self.await_thread_started().await?
            }
        };

        let mut user_input = Vec::with_capacity(1 + input.image_paths.len());
        user_input.push(serde_json::json!({
            "type": "text",
            "text": input.text,
            "text_elements": [],
        }));
        user_input.extend(input.image_paths.into_iter().map(|path| {
            serde_json::json!({
                "type": "localImage",
                "path": path,
            })
        }));

        let mut params = serde_json::json!({
            "threadId": thread_id,
            "input": user_input,
        });
        if let Some(selection) = &self.model_selection {
            params["model"] = serde_json::json!(selection.model);
            params["effort"] = serde_json::json!(selection.reasoning_effort);
        }
        if let Some(output_schema) = input.output_schema {
            params["outputSchema"] = output_schema;
        }
        if let Some(workspace_root) = input.workspace_root.as_deref() {
            params["cwd"] = serde_json::json!(path_text(workspace_root)?);
        }
        let started = self.send_request("turn/start", params).await?;
        let turn_id = started
            .pointer("/turn/id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        if *cancellation.borrow() != cancellation_generation {
            if turn_completed.try_recv().is_ok() {
                return Ok(false);
            }
            let turn_id = turn_id.as_deref().ok_or_else(|| {
                AppServerError::new("turn/start response omitted turn.id; cannot interrupt")
            })?;
            self.send_request(
                "turn/interrupt",
                serde_json::json!({"threadId": thread_id, "turnId": turn_id}),
            )
            .await?;
            turn_completed.recv().await.map_err(|err| {
                AppServerError::new(format!("interrupted turn completion wait failed: {err}"))
            })?;
            return Ok(true);
        }

        tokio::select! {
            biased;
            completed = turn_completed.recv() => {
                completed.map_err(|err| {
                    AppServerError::new(format!("turn completion wait failed: {err}"))
                })?;
                Ok(false)
            }
            changed = cancellation.changed() => {
                if changed.is_err() {
                    turn_completed.recv().await.map_err(|err| {
                        AppServerError::new(format!("turn completion wait failed: {err}"))
                    })?;
                    return Ok(false);
                }
                let turn_id = turn_id.as_deref().ok_or_else(|| {
                    AppServerError::new("turn/start response omitted turn.id; cannot interrupt")
                })?;
                self.send_request(
                    "turn/interrupt",
                    serde_json::json!({"threadId": thread_id, "turnId": turn_id}),
                )
                .await?;
                turn_completed.recv().await.map_err(|err| {
                    AppServerError::new(format!("interrupted turn completion wait failed: {err}"))
                })?;
                Ok(true)
            }
        }
    }

    pub async fn current_thread_id(&self) -> Option<String> {
        self.thread_id.lock().await.clone()
    }

    /// Seed the client's thread id so the NEXT `run_turn` issues `thread/resume`
    /// against `id` (session restore: the driver injects the saved thread id).
    pub async fn set_thread_id(&self, id: String) {
        *self.thread_id.lock().await = Some(id);
    }

    async fn await_thread_started(&self) -> Result<String, AppServerError> {
        loop {
            if let Some(thread_id) = self.current_thread_id().await {
                return Ok(thread_id);
            }
            self.thread_started.notified().await;
        }
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppServerError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| AppServerError::new("JSON-RPC request id overflow"))?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        if let Err(err) = self.write_message(request).await {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }

        rx.await.map_err(|err| {
            self.transport.mark_closed();
            AppServerError::new(
                self.transport
                    .failure_message(&format!("response channel closed: {err}")),
            )
        })?
    }
}

/// Config overrides shared by read and write app-server processes. The active
/// default profile is appended by [`app_server_config_overrides`].
pub(crate) const APP_SERVER_CONFIG_OVERRIDES: [&str; 7] = [
    "skills.include_instructions=false",
    "project_doc_max_bytes=0",
    "model_supports_reasoning_summaries=true",
    "model_reasoning_summary=\"detailed\"",
    "windows.sandbox=\"elevated\"",
    "permissions.eud_workspace_read={description=\"eud-agent read-only session workspace\",filesystem={\":minimal\"=\"read\",\":workspace_roots\"={\".\"=\"read\"}},network={enabled=false}}",
    "permissions.eud_workspace_write={description=\"eud-agent write-owner session workspace\",filesystem={\":minimal\"=\"read\",\":workspace_roots\"={\".\"=\"write\",\"source/**\"=\"read\"}},network={enabled=false}}",
];

pub(crate) fn app_server_config_overrides(access: WorkspaceAccess) -> Vec<String> {
    let mut overrides = APP_SERVER_CONFIG_OVERRIDES
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    let profile = match access {
        WorkspaceAccess::Read => "eud_workspace_read",
        WorkspaceAccess::Write => "eud_workspace_write",
    };
    overrides.push(format!("default_permissions=\"{profile}\""));
    overrides
}

/// Params for a fresh thread. Project turns use the custom split-filesystem
/// profile selected at app-server launch; tests/non-project callers retain the
/// previous read-only policy. The session MCP config is repeated here because a
/// resumed Codex thread can retain the config stored when that thread was created.
fn thread_start_params(
    workspace_root: Option<&Path>,
    mcp_server_url: Option<&str>,
    large_context_enabled: bool,
) -> Result<serde_json::Value, AppServerError> {
    let mut params = serde_json::json!({ "approvalPolicy": "on-request" });
    if let Some(workspace_root) = workspace_root {
        params["cwd"] = serde_json::json!(path_text(workspace_root)?);
    } else {
        params["sandboxPolicy"] = serde_json::json!({ "type": "readOnly", "networkAccess": false });
    }
    if let Some(config) = thread_config(mcp_server_url, large_context_enabled) {
        params["config"] = config;
    }
    Ok(params)
}

fn thread_resume_params(
    thread_id: &str,
    workspace_root: Option<&Path>,
    mcp_server_url: Option<&str>,
    large_context_enabled: bool,
) -> Result<serde_json::Value, AppServerError> {
    let mut params = serde_json::json!({ "threadId": thread_id });
    if let Some(workspace_root) = workspace_root {
        params["cwd"] = serde_json::json!(path_text(workspace_root)?);
    }
    if let Some(config) = thread_config(mcp_server_url, large_context_enabled) {
        params["config"] = config;
    }
    Ok(params)
}

fn thread_config(
    mcp_server_url: Option<&str>,
    large_context_enabled: bool,
) -> Option<serde_json::Value> {
    let mut config = serde_json::Map::new();
    if let Some(url) = mcp_server_url {
        config.insert(
            "mcp_servers".to_string(),
            serde_json::json!({
                "eud-tools": {
                    "url": url,
                },
            }),
        );
    }
    if large_context_enabled {
        config.insert(
            "model_context_window".to_string(),
            serde_json::json!(LARGE_CONTEXT_WINDOW_TOKENS),
        );
        config.insert(
            "model_auto_compact_token_limit".to_string(),
            serde_json::json!(LARGE_CONTEXT_AUTO_COMPACT_TOKENS),
        );
    }
    (!config.is_empty()).then_some(serde_json::Value::Object(config))
}

fn path_text(path: &Path) -> Result<String, AppServerError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| AppServerError::new("Codex workspace path is not UTF-8"))
}

fn mcp_server_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// The dotted-key `-c` override that registers the in-process eud-tools MCP
/// server with codex over loopback streamable HTTP (decision A2). The value is a
/// TOML string, so it is quoted; the `eud-tools` key segment is a valid TOML
/// bare key (hyphens allowed). codex's `RawMcpServerConfig` selects the HTTP
/// transport from the presence of `url`.
pub(crate) fn mcp_server_override(url: &str) -> String {
    format!("mcp_servers.eud-tools.url=\"{url}\"")
}

impl CodexAppServerClient<tokio::process::ChildStdout, tokio::process::ChildStdin> {
    pub async fn spawn_app_server(
        cwd: impl AsRef<std::path::Path>,
        mcp_port: Option<u16>,
        access: WorkspaceAccess,
        ask_runtime: Option<crate::tool_exec::SessionToolRuntime>,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<AppServerEvent>), AppServerError> {
        let codex_cmd = resolve_codex_cmd().map_err(|err| AppServerError::new(err.to_string()))?;
        let mcp_server_url = mcp_port.map(mcp_server_url);
        let mut command = tokio::process::Command::new(codex_cmd);
        command.arg("app-server");
        for override_arg in app_server_config_overrides(access) {
            command.arg("-c").arg(override_arg);
        }
        // Launch-level registration covers fresh threads; thread/start and
        // thread/resume repeat it so restored threads cannot retain stale tools.
        if let Some(url) = mcp_server_url.as_deref() {
            command.arg("-c").arg(mcp_server_override(url));
        }
        let private_tmp = cwd.as_ref().join(crate::workspace::TEMP_DIR);
        if private_tmp.is_dir() {
            command.env("TEMP", &private_tmp).env("TMP", &private_tmp);
        }
        command
            .current_dir(cwd.as_ref())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let mut child = command.spawn().map_err(|err| {
            AppServerError::new(format!("failed to spawn codex app-server: {err}"))
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppServerError::new("codex app-server stdout was not piped"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppServerError::new("codex app-server stdin was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppServerError::new("codex app-server stderr was not piped"))?;

        let (mut client, events) = Self::new_with_stdio_and_ask_runtime(stdout, stdin, ask_runtime);
        client.mcp_server_url = mcp_server_url;
        let waiter_transport = std::sync::Arc::clone(&client.transport);
        let waiter = tokio::spawn(async move {
            waiter_transport.record_exit(child.wait().await);
        });
        let stderr_transport = std::sync::Arc::clone(&client.transport);
        let stderr_reader = tokio::spawn(read_app_server_stderr(stderr, stderr_transport));
        client._process = Some(AppServerProcess {
            waiter,
            stderr_reader,
        });
        Ok((client, events))
    }
}

async fn read_app_server_stdout<R, W>(reader: R, context: AppServerReadContext<W>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let AppServerReadContext {
        writer,
        pending,
        events_tx,
        thread_id,
        thread_started,
        turn_completed,
        transport,
        ask_runtime,
    } = context;
    use tokio::io::AsyncBufReadExt as _;

    let mut close_reason = "app-server stdout closed".to_string();
    let mut lines = tokio::io::BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(err) => {
                close_reason = format!("failed reading app-server stdout: {err}");
                break;
            }
        };

        let message = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(message) => message,
            Err(err) => {
                if events_tx
                    .send(AppServerEvent::Error(format!(
                        "failed parsing app-server JSON-RPC line: {err}"
                    )))
                    .await
                    .is_err()
                {
                    close_reason = "app-server event receiver closed".to_string();
                    break;
                }
                continue;
            }
        };

        let method = message.get("method").and_then(serde_json::Value::as_str);
        let id = message.get("id").cloned();

        match (method, id) {
            (Some(method), Some(id)) => {
                if let Err(error) = handle_server_request(
                    &writer,
                    method,
                    id,
                    message.get("params"),
                    ask_runtime.as_ref(),
                )
                .await
                {
                    close_reason = error.message;
                    break;
                }
            }
            (Some(method), None) => {
                let should_continue = handle_notification(
                    method,
                    message.get("params"),
                    &events_tx,
                    &thread_id,
                    &thread_started,
                    &turn_completed,
                )
                .await;
                if !should_continue {
                    close_reason = "app-server event receiver closed".to_string();
                    break;
                }
            }
            (None, Some(id)) => {
                complete_pending_request(&pending, id, &message).await;
            }
            (None, None) => {}
        }
    }

    transport.mark_closed();
    transport.wait_for_exit_detail().await;
    let failure = transport.failure_message(&close_reason);
    let _ = events_tx.try_send(AppServerEvent::Error(failure.clone()));
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(AppServerError::new(failure.clone())));
    }
}

async fn read_app_server_stderr(
    mut stderr: tokio::process::ChildStderr,
    transport: std::sync::Arc<AppServerTransportState>,
) {
    use tokio::io::AsyncReadExt as _;

    let mut buffer = [0_u8; 1024];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => return,
            Ok(read) => transport.append_stderr(&buffer[..read]),
            Err(error) => {
                transport.append_stderr(format!("stderr read failed: {error}").as_bytes());
                return;
            }
        }
    }
}

async fn complete_pending_request(
    pending: &AppServerPending,
    id: serde_json::Value,
    message: &serde_json::Value,
) {
    let Some(id) = id.as_u64() else {
        return;
    };
    let result = if let Some(error) = message.get("error") {
        Err(AppServerError::new(format!(
            "app-server request failed: {error}"
        )))
    } else {
        Ok(message
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    };

    if let Some(tx) = pending.lock().await.remove(&id) {
        let _ = tx.send(result);
    }
}

async fn handle_server_request<W>(
    writer: &AppServerWriter<W>,
    method: &str,
    id: serde_json::Value,
    params: Option<&serde_json::Value>,
    ask_runtime: Option<&crate::tool_exec::SessionToolRuntime>,
) -> Result<(), AppServerError>
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let result = if let Some(args) = eud_ask_arguments(method, params) {
        match ask_runtime {
            Some(runtime) => match runtime.ask(&args).await {
                Ok(value) => {
                    let payload = serde_json::to_string(&value).expect("JSON value must serialize");
                    let content = serde_json::Map::from_iter([(
                        crate::mcp::ASK_ELICITATION_PAYLOAD_KEY.to_string(),
                        serde_json::Value::String(payload),
                    )]);
                    serde_json::json!({ "action": "accept", "content": content })
                }
                Err(_) => serde_json::json!({ "action": "cancel", "content": null }),
            },
            None => serde_json::json!({ "action": "decline", "content": null }),
        }
    } else {
        match method {
            "item/commandExecution/requestApproval" => {
                serde_json::json!({ "decision": "accept" })
            }
            "execCommandApproval" => serde_json::json!({ "decision": "approved" }),
            _ if should_accept_mcp_elicitation(method, params) => {
                serde_json::json!({ "action": "accept", "content": null })
            }
            _ => decline_approval_result(method),
        }
    };

    write_json_rpc_line(
        writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
    .await
}

fn decline_approval_result(method: &str) -> serde_json::Value {
    match method {
        "mcpServer/elicitation/request" => {
            serde_json::json!({ "action": "decline", "content": null })
        }
        "item/fileChange/requestApproval" | "item/permissions/requestApproval" => {
            serde_json::json!({ "decision": "decline" })
        }
        "applyPatchApproval" => serde_json::json!({ "decision": "denied" }),
        _ => serde_json::json!({ "decision": "decline" }),
    }
}

fn eud_ask_arguments(
    method: &str,
    params: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    if method != "mcpServer/elicitation/request" {
        return None;
    }
    let params = params?;
    if !is_eud_tools_request(params) {
        return None;
    }
    params
        .get("_meta")
        .or_else(|| params.pointer("/request/_meta"))?
        .get(crate::mcp::ASK_ELICITATION_META_KEY)
        .cloned()
}

fn is_eud_tools_request(params: &serde_json::Value) -> bool {
    ["server", "serverName", "server_name", "name"]
        .iter()
        .any(|key| params.get(*key).and_then(serde_json::Value::as_str) == Some("eud-tools"))
}

fn should_accept_mcp_elicitation(method: &str, params: Option<&serde_json::Value>) -> bool {
    if method != "mcpServer/elicitation/request" {
        return false;
    }

    let Some(params) = params else {
        return false;
    };

    let approval_kind = params
        .get("_meta")
        .and_then(|meta| meta.get("codex_approval_kind"))
        .and_then(serde_json::Value::as_str);
    if approval_kind != Some("mcp_tool_call") {
        return false;
    }

    is_eud_tools_request(params)
}

async fn handle_notification(
    method: &str,
    params: Option<&serde_json::Value>,
    events_tx: &tokio::sync::mpsc::Sender<AppServerEvent>,
    thread_id: &std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    thread_started: &std::sync::Arc<tokio::sync::Notify>,
    turn_completed: &tokio::sync::broadcast::Sender<()>,
) -> bool {
    match method {
        "thread/started" => {
            let Some(id) = params
                .and_then(|params| params.get("thread"))
                .and_then(|thread| thread.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| string_param(params, &["threadId", "thread_id", "id"]))
            else {
                return true;
            };
            *thread_id.lock().await = Some(id.clone());
            thread_started.notify_waiters();
            send_event(events_tx, AppServerEvent::ThreadStarted { thread_id: id }).await
        }
        "thread/tokenUsage/updated" => {
            if let Some(event) = token_usage_event(params) {
                send_event(events_tx, event).await
            } else {
                true
            }
        }
        "turn/started" => send_event(events_tx, AppServerEvent::TurnStarted).await,
        "item/agentMessage/delta" => {
            if let Some(delta) = string_param(params, &["delta"]) {
                send_event(events_tx, AppServerEvent::AnswerDelta(delta)).await
            } else {
                true
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let Some(delta) = string_param(params, &["delta"]) {
                send_event(events_tx, AppServerEvent::ReasoningDelta(delta)).await
            } else {
                true
            }
        }
        "item/started" => {
            // Tool-like items (mcpToolCall / commandExecution / webSearch) map to a
            // ToolCallStarted carrying the tool name + args so the panel renders a
            // Tool card (EUD-068); everything else keeps the bare item signal.
            let event = context_compaction_event(params, false)
                .or_else(|| tool_event_from_item(params, false))
                .unwrap_or_else(|| AppServerEvent::ItemStarted {
                    item_id: string_param(params, &["itemId", "item_id", "id"]),
                });
            send_event(events_tx, event).await
        }
        "item/completed" => {
            let event = context_compaction_event(params, true)
                .or_else(|| tool_event_from_item(params, true))
                .unwrap_or_else(|| AppServerEvent::ItemCompleted {
                    item_id: string_param(params, &["itemId", "item_id", "id"]),
                });
            send_event(events_tx, event).await
        }
        "turn/completed" => {
            let should_continue = send_event(events_tx, AppServerEvent::TurnComplete).await;
            let _ = turn_completed.send(());
            should_continue
        }
        "error" => {
            let message = string_param(params, &["message"])
                .or_else(|| {
                    params
                        .and_then(|params| params.get("error"))
                        .and_then(|error| error.get("message"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "app-server error".to_string());
            send_event(events_tx, AppServerEvent::Error(message)).await
        }
        _ => true,
    }
}
fn context_compaction_event(
    params: Option<&serde_json::Value>,
    completed: bool,
) -> Option<AppServerEvent> {
    if params?.pointer("/item/type")?.as_str()? != "contextCompaction" {
        return None;
    }
    Some(if completed {
        AppServerEvent::ContextCompactionCompleted
    } else {
        AppServerEvent::ContextCompactionStarted
    })
}

async fn send_event(
    events_tx: &tokio::sync::mpsc::Sender<AppServerEvent>,
    event: AppServerEvent,
) -> bool {
    events_tx.send(event).await.is_ok()
}

fn string_param(params: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let params = params?;
    keys.iter()
        .find_map(|key| params.get(*key).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

fn token_usage_event(params: Option<&serde_json::Value>) -> Option<AppServerEvent> {
    let params = params?;
    let turn_id = params.get("turnId")?.as_str()?.to_string();
    let token_usage: ipc::ContextUsage =
        serde::Deserialize::deserialize(params.get("tokenUsage")?).ok()?;
    Some(AppServerEvent::TokenUsageUpdated {
        turn_id,
        token_usage,
    })
}

/// Cap on tool args/result text relayed to the panel (panel render safety,
/// EUD-068 — same budget + marker as the verified v1 server).
const TOOL_DATA_MAX_CHARS: usize = 4000;

fn truncate_tool_text(text: String) -> String {
    if text.chars().count() <= TOOL_DATA_MAX_CHARS {
        return text;
    }
    let mut out: String = text.chars().take(TOOL_DATA_MAX_CHARS).collect();
    out.push_str(" …(잘림)");
    out
}

/// Read a field accepting both the official camelCase key and a snake_case
/// fallback (the SDK observed camelCase, EUD-053; defensive on both).
fn item_field<'a>(item: &'a serde_json::Value, keys: &[&str]) -> Option<&'a serde_json::Value> {
    keys.iter().find_map(|key| item.get(*key))
}

/// Tool-call argument text: a string value passes through; anything else is
/// dumped as compact JSON (EUD-068 `_tool_args_text`).
fn tool_args_text(value: &serde_json::Value) -> String {
    let text = match value.as_str() {
        Some(s) => s.to_string(),
        None => value.to_string(),
    };
    truncate_tool_text(text)
}

/// Tool result text: the error message on failure, else the joined MCP content
/// text blocks, else the compact JSON of the result (EUD-068
/// `_tool_result_data`).
fn tool_result_text(item: &serde_json::Value) -> Option<String> {
    if let Some(error) = item_field(item, &["error"]) {
        if !error.is_null() {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| match error.as_str() {
                    Some(s) => s.to_string(),
                    None => error.to_string(),
                });
            return Some(truncate_tool_text(message));
        }
    }
    let result = item_field(item, &["result"])?;
    if result.is_null() {
        return None;
    }
    let joined = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|joined| !joined.is_empty());
    let text = joined.unwrap_or_else(|| match result.as_str() {
        Some(s) => s.to_string(),
        None => result.to_string(),
    });
    Some(truncate_tool_text(text))
}

/// Classify a thread item from `item/started|completed` params as a
/// user-visible tool call (EUD-068 v1 `_classify_event`, ported to v2): an
/// `mcpToolCall` renders by its MCP tool name with the call arguments; a
/// `commandExecution` by its command line; a `webSearch` by its query. Any
/// other item type returns None and keeps the bare item_started/item_completed
/// signal (which the panel intentionally ignores).
fn tool_event_from_item(
    params: Option<&serde_json::Value>,
    completed: bool,
) -> Option<AppServerEvent> {
    let item = params?.get("item")?;
    let item_type = item.get("type").and_then(serde_json::Value::as_str)?;
    let (name, args, result) = match item_type {
        "mcpToolCall" | "mcp_tool_call" => {
            let name = item_field(item, &["tool"])
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let args = item_field(item, &["arguments"])
                .filter(|value| !value.is_null())
                .map(tool_args_text);
            (name, args, tool_result_text(item))
        }
        "commandExecution" | "command_execution" => {
            let args = item_field(item, &["command"])
                .and_then(serde_json::Value::as_str)
                .map(|command| truncate_tool_text(command.to_string()));
            let result = item_field(item, &["aggregatedOutput", "aggregated_output"])
                .and_then(serde_json::Value::as_str)
                .filter(|output| !output.is_empty())
                .map(|output| truncate_tool_text(output.to_string()))
                .or_else(|| tool_result_text(item));
            ("command".to_string(), args, result)
        }
        "webSearch" | "web_search" => {
            let args = item_field(item, &["query"])
                .and_then(serde_json::Value::as_str)
                .map(|query| truncate_tool_text(query.to_string()));
            ("web_search".to_string(), args, tool_result_text(item))
        }
        _ => return None,
    };
    if completed {
        Some(AppServerEvent::ToolCallCompleted {
            name,
            result,
            status: item_field(item, &["status"])
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
    } else {
        Some(AppServerEvent::ToolCallStarted { name, args })
    }
}

async fn write_json_rpc_line<W>(
    writer: &AppServerWriter<W>,
    value: serde_json::Value,
) -> Result<(), AppServerError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt as _;

    let mut writer = writer.lock().await;
    writer
        .write_all(value.to_string().as_bytes())
        .await
        .map_err(|err| AppServerError::new(format!("failed writing JSON-RPC line: {err}")))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|err| AppServerError::new(format!("failed writing JSON-RPC newline: {err}")))?;
    writer
        .flush()
        .await
        .map_err(|err| AppServerError::new(format!("failed flushing JSON-RPC line: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod app_server_override_tests {
    //! Pins the hermetic-turn config overrides: user-local skills and any
    //! AGENTS.md found at/above the cwd must never steer the agent's codex
    //! turns (only our system prompt does).
    use super::{app_server_config_overrides, WorkspaceAccess, APP_SERVER_CONFIG_OVERRIDES};

    #[test]
    fn well_known_codex_distribution_is_under_the_local_app_data_bin_dir() {
        let local = std::path::Path::new("C:\\Users\\x\\AppData\\Local");
        let codex = super::well_known_codex_path(local);
        let host = super::well_known_codex_host_path(local);
        let sandbox_setup = super::well_known_codex_sandbox_setup_path(local);
        assert!(
            codex.ends_with("eud-agent\\bin\\codex.exe")
                || codex.ends_with("eud-agent/bin/codex.exe")
        );
        assert!(
            host.ends_with("eud-agent\\bin\\codex-code-mode-host.exe")
                || host.ends_with("eud-agent/bin/codex-code-mode-host.exe")
        );
        assert!(
            sandbox_setup.ends_with("eud-agent\\bin\\codex-windows-sandbox-setup.exe")
                || sandbox_setup.ends_with("eud-agent/bin/codex-windows-sandbox-setup.exe")
        );
    }

    #[test]
    fn app_managed_codex_requires_its_runtime_siblings() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let local = std::env::temp_dir().join(format!("eud-agent-codex-path-test-{nanos}"));
        let bin = local.join("eud-agent").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex.exe"), b"codex").unwrap();

        assert_eq!(
            super::app_managed_codex(&local),
            Err(bin.join("codex-code-mode-host.exe"))
        );

        std::fs::write(bin.join("codex-code-mode-host.exe"), b"host").unwrap();
        assert_eq!(
            super::app_managed_codex(&local),
            Err(bin.join("codex-windows-sandbox-setup.exe"))
        );

        std::fs::write(
            bin.join("codex-windows-sandbox-setup.exe"),
            b"sandbox setup",
        )
        .unwrap();
        assert_eq!(
            super::app_managed_codex(&local),
            Ok(Some(bin.join("codex.exe")))
        );
        std::fs::remove_dir_all(local).ok();
    }

    #[test]
    fn overrides_define_distinct_strict_workspace_profiles() {
        assert!(APP_SERVER_CONFIG_OVERRIDES.contains(&"skills.include_instructions=false"));
        assert!(APP_SERVER_CONFIG_OVERRIDES.contains(&"project_doc_max_bytes=0"));
        assert!(APP_SERVER_CONFIG_OVERRIDES.contains(&"windows.sandbox=\"elevated\""));
        assert!(APP_SERVER_CONFIG_OVERRIDES
            .iter()
            .any(|value| value.starts_with("permissions.eud_workspace_read=")));
        assert!(APP_SERVER_CONFIG_OVERRIDES
            .iter()
            .any(|value| value.starts_with("permissions.eud_workspace_write=")));
        let read = app_server_config_overrides(WorkspaceAccess::Read);
        let write = app_server_config_overrides(WorkspaceAccess::Write);
        assert!(read
            .iter()
            .any(|value| value == "default_permissions=\"eud_workspace_read\""));
        assert!(write
            .iter()
            .any(|value| value == "default_permissions=\"eud_workspace_write\""));
    }

    #[test]
    fn mcp_server_override_is_a_loopback_dotted_key_with_a_quoted_url() {
        // codex selects the streamable-HTTP transport from `url`; the value is a
        // TOML string (quoted), the key segment `eud-tools` is a valid bare key.
        let url = super::mcp_server_url(54321);
        let arg = super::mcp_server_override(&url);
        assert_eq!(
            arg,
            "mcp_servers.eud-tools.url=\"http://127.0.0.1:54321/mcp\""
        );
    }

    #[test]
    fn thread_start_uses_readonly_without_a_project_workspace() {
        let params = super::thread_start_params(None, None, false).unwrap();
        assert_eq!(
            params["sandboxPolicy"]["type"],
            serde_json::json!("readOnly")
        );
        assert_eq!(
            params["sandboxPolicy"]["networkAccess"],
            serde_json::json!(false)
        );
        assert_eq!(params["approvalPolicy"], serde_json::json!("on-request"));
    }

    #[test]
    fn thread_start_uses_custom_profile_for_project_workspace() {
        let params =
            super::thread_start_params(Some(std::path::Path::new("C:\\workspace")), None, false)
                .unwrap();
        assert_eq!(params["cwd"], serde_json::json!("C:\\workspace"));
        assert!(params.get("sandboxPolicy").is_none());
    }
    #[test]
    fn large_context_opt_in_sets_native_window_and_auto_compaction_limit() {
        let params =
            super::thread_start_params(None, Some("http://127.0.0.1:54321/mcp"), true).unwrap();
        assert_eq!(
            params.pointer("/config/model_context_window"),
            Some(&serde_json::json!(1_000_000))
        );
        assert_eq!(
            params.pointer("/config/model_auto_compact_token_limit"),
            Some(&serde_json::json!(900_000))
        );
        assert_eq!(
            params.pointer("/config/mcp_servers/eud-tools/url"),
            Some(&serde_json::json!("http://127.0.0.1:54321/mcp"))
        );
    }
}

#[cfg(test)]
mod compaction_item_tests {
    use super::{context_compaction_event, AppServerEvent};
    use serde_json::json;

    #[test]
    fn native_context_compaction_items_have_distinct_lifecycle_events() {
        let params = json!({
            "threadId": "thread-1",
            "item": {"id": "compact-1", "type": "contextCompaction"}
        });
        assert_eq!(
            context_compaction_event(Some(&params), false),
            Some(AppServerEvent::ContextCompactionStarted)
        );
        assert_eq!(
            context_compaction_event(Some(&params), true),
            Some(AppServerEvent::ContextCompactionCompleted)
        );
        assert_eq!(
            context_compaction_event(
                Some(&json!({"item": {"id": "message-1", "type": "agentMessage"}})),
                false,
            ),
            None
        );
    }
}

#[cfg(test)]
mod tool_item_tests {
    //! EUD-068 classification port (v2 regression: item/started|completed
    //! dropped the item payload, so MCP tool calls never rendered as Tool
    //! cards). Pins the item → ToolCallStarted/Completed mapping.
    use super::{tool_event_from_item, AppServerEvent, TOOL_DATA_MAX_CHARS};
    use serde_json::json;

    #[test]
    fn mcp_tool_call_started_maps_to_tool_call_with_args() {
        let params = json!({
            "item": {
                "id": "item_1",
                "type": "mcpToolCall",
                "server": "eud-tools",
                "tool": "search_docs",
                "arguments": {"query": "countdown"},
                "status": "inProgress"
            }
        });
        let event = tool_event_from_item(Some(&params), false);
        assert_eq!(
            event,
            Some(AppServerEvent::ToolCallStarted {
                name: "search_docs".to_string(),
                args: Some("{\"query\":\"countdown\"}".to_string()),
            })
        );
    }

    #[test]
    fn mcp_tool_call_completed_joins_content_text_and_keeps_status() {
        let params = json!({
            "item": {
                "id": "item_1",
                "type": "mcpToolCall",
                "tool": "search_docs",
                "status": "completed",
                "result": {"content": [
                    {"type": "text", "text": "hit 1"},
                    {"type": "text", "text": "hit 2"}
                ]}
            }
        });
        let event = tool_event_from_item(Some(&params), true);
        assert_eq!(
            event,
            Some(AppServerEvent::ToolCallCompleted {
                name: "search_docs".to_string(),
                result: Some("hit 1\nhit 2".to_string()),
                status: Some("completed".to_string()),
            })
        );
    }

    #[test]
    fn mcp_tool_call_failure_prefers_the_error_message() {
        let params = json!({
            "item": {
                "type": "mcpToolCall",
                "tool": "dat_set",
                "status": "failed",
                "error": {"message": "EvidenceRequired"},
                "result": {"content": []}
            }
        });
        let event = tool_event_from_item(Some(&params), true);
        assert_eq!(
            event,
            Some(AppServerEvent::ToolCallCompleted {
                name: "dat_set".to_string(),
                result: Some("EvidenceRequired".to_string()),
                status: Some("failed".to_string()),
            })
        );
    }

    #[test]
    fn command_execution_maps_command_and_aggregated_output() {
        let started = json!({
            "item": {"type": "commandExecution", "command": "cargo test", "status": "inProgress"}
        });
        assert_eq!(
            tool_event_from_item(Some(&started), false),
            Some(AppServerEvent::ToolCallStarted {
                name: "command".to_string(),
                args: Some("cargo test".to_string()),
            })
        );

        let completed = json!({
            "item": {
                "type": "commandExecution",
                "command": "cargo test",
                "aggregatedOutput": "ok. 12 passed",
                "exitCode": 0,
                "status": "completed"
            }
        });
        assert_eq!(
            tool_event_from_item(Some(&completed), true),
            Some(AppServerEvent::ToolCallCompleted {
                name: "command".to_string(),
                result: Some("ok. 12 passed".to_string()),
                status: Some("completed".to_string()),
            })
        );
    }

    #[test]
    fn non_tool_items_return_none_so_the_bare_item_signal_is_kept() {
        for item_type in ["agentMessage", "reasoning", "fileChange", "todoList"] {
            let params = json!({ "item": {"type": item_type, "id": "item_9"} });
            assert_eq!(tool_event_from_item(Some(&params), false), None);
            assert_eq!(tool_event_from_item(Some(&params), true), None);
        }
        assert_eq!(tool_event_from_item(None, false), None);
        assert_eq!(tool_event_from_item(Some(&json!({})), false), None);
    }

    #[test]
    fn oversized_args_truncate_with_the_marker() {
        let big = "x".repeat(TOOL_DATA_MAX_CHARS + 10);
        let params = json!({
            "item": {"type": "mcpToolCall", "tool": "t", "arguments": big}
        });
        let Some(AppServerEvent::ToolCallStarted {
            args: Some(args), ..
        }) = tool_event_from_item(Some(&params), false)
        else {
            panic!("expected a ToolCallStarted with args");
        };
        assert_eq!(
            args.chars().count(),
            TOOL_DATA_MAX_CHARS + " …(잘림)".chars().count()
        );
        assert!(args.ends_with("…(잘림)"));
    }
}

#[cfg(test)]
mod token_usage_tests {
    use super::{token_usage_event, AppServerEvent};
    use crate::ipc::{ContextUsage, TokenUsageBreakdown};
    use serde_json::json;

    #[test]
    fn notification_maps_active_and_cumulative_context_usage() {
        let params = json!({
            "threadId": "thread-1",
            "turnId": "turn-2",
            "tokenUsage": {
                "last": {
                    "inputTokens": 31_000,
                    "cachedInputTokens": 24_000,
                    "outputTokens": 1_200,
                    "reasoningOutputTokens": 800,
                    "totalTokens": 32_200
                },
                "total": {
                    "inputTokens": 52_000,
                    "cachedInputTokens": 40_000,
                    "cacheWriteInputTokens": 600,
                    "outputTokens": 2_100,
                    "reasoningOutputTokens": 1_300,
                    "totalTokens": 54_100
                },
                "modelContextWindow": 128_000
            }
        });

        assert_eq!(
            token_usage_event(Some(&params)),
            Some(AppServerEvent::TokenUsageUpdated {
                turn_id: "turn-2".to_string(),
                token_usage: ContextUsage {
                    last: TokenUsageBreakdown {
                        input_tokens: 31_000,
                        cached_input_tokens: 24_000,
                        cache_write_input_tokens: 0,
                        output_tokens: 1_200,
                        reasoning_output_tokens: 800,
                        total_tokens: 32_200,
                    },
                    total: TokenUsageBreakdown {
                        input_tokens: 52_000,
                        cached_input_tokens: 40_000,
                        cache_write_input_tokens: 600,
                        output_tokens: 2_100,
                        reasoning_output_tokens: 1_300,
                        total_tokens: 54_100,
                    },
                    model_context_window: Some(128_000),
                },
            })
        );
    }

    #[test]
    fn malformed_notification_is_ignored() {
        assert_eq!(
            token_usage_event(Some(&json!({
                "turnId": "turn-2",
                "tokenUsage": {"last": {}, "total": {}}
            }))),
            None
        );
    }
}

#[cfg(test)]
mod appserver_tests {
    use super::{
        AppServerEvent, CodexAppServerClient, CodexModelSelection, CodexTurnInput, WorkspaceAccess,
    };
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

    async fn read_json_line(lines: &mut Lines<BufReader<DuplexStream>>) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("timed out waiting for JSON-RPC line")
            .expect("failed reading JSON-RPC line")
            .expect("peer closed before sending JSON-RPC line");

        serde_json::from_str(&line).expect("line must be valid JSON")
    }

    async fn write_json_line(writer: &mut DuplexStream, value: Value) {
        writer
            .write_all(value.to_string().as_bytes())
            .await
            .expect("failed writing JSON-RPC line");
        writer
            .write_all(b"\n")
            .await
            .expect("failed writing JSON-RPC newline");
        writer.flush().await.expect("failed flushing JSON-RPC line");
    }

    fn assert_client_request(value: &Value, method: &str) -> Value {
        assert_eq!(value.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(value.get("method").and_then(Value::as_str), Some(method));
        assert!(
            value.get("id").is_some(),
            "{method} must be sent as a JSON-RPC request"
        );
        assert!(
            value.get("params").is_some(),
            "{method} must include params"
        );
        value["id"].clone()
    }

    /// thread/start MUST clamp the codex subprocess to a read-only, offline
    /// sandbox so its own shell/apply_patch tools cannot write the disk (the
    /// project is the editor's map via eud-tools, never the cwd).
    fn assert_thread_start_sandbox(value: &Value) {
        let sandbox = &value["params"]["sandboxPolicy"];
        assert_eq!(
            sandbox.get("type").and_then(Value::as_str),
            Some("readOnly"),
            "thread/start must request the readOnly sandbox, got {sandbox:?}"
        );
        assert_eq!(
            sandbox.get("networkAccess").and_then(Value::as_bool),
            Some(false),
            "thread/start sandbox must keep the shell offline, got {sandbox:?}"
        );
        // The eud-tools MCP elicitation accept flow rides the approval handler.
        assert_eq!(
            value["params"]
                .get("approvalPolicy")
                .and_then(Value::as_str),
            Some("on-request"),
            "thread/start must keep approvalPolicy on-request for MCP elicitations"
        );
    }

    fn assert_eud_tools_config(value: &Value, expected_url: &str) {
        assert_eq!(
            value
                .pointer("/params/config/mcp_servers/eud-tools/url")
                .and_then(Value::as_str),
            Some(expected_url),
            "thread start/resume must inject the live session MCP endpoint"
        );
    }

    fn assert_initialize_params(value: &Value) {
        assert_eq!(
            value
                .pointer("/params/clientInfo/name")
                .and_then(Value::as_str),
            Some("eud-agent")
        );
    }
    fn assert_initialized_notification(value: &Value) {
        assert_eq!(value, &json!({ "method": "initialized" }));
    }

    fn assert_prompt(value: &Value, expected: &str) {
        let params = value
            .get("params")
            .and_then(Value::as_object)
            .expect("request params must be an object");
        let serialized = serde_json::to_string(params).expect("params serialize");
        assert!(
            serialized.contains(expected),
            "request params should carry prompt {expected:?}, got {serialized}"
        );
        assert_eq!(
            value
                .pointer("/params/input/0/type")
                .and_then(Value::as_str),
            Some("text")
        );
    }

    fn assert_local_image(value: &Value, expected_path: &str) {
        assert_eq!(
            value
                .pointer("/params/input/1/type")
                .and_then(Value::as_str),
            Some("localImage")
        );
        assert_eq!(
            value
                .pointer("/params/input/1/path")
                .and_then(Value::as_str),
            Some(expected_path)
        );
    }

    fn assert_thread_id(value: &Value, expected: &str) {
        let params = value
            .get("params")
            .and_then(Value::as_object)
            .expect("request params must be an object");
        let serialized = serde_json::to_string(params).expect("params serialize");
        assert!(
            serialized.contains(expected),
            "thread/resume params should reuse thread id {expected:?}, got {serialized}"
        );
    }

    fn assert_turn_thread_id(value: &Value, expected: &str) {
        assert_eq!(
            value.pointer("/params/threadId").and_then(Value::as_str),
            Some(expected)
        );
    }
    fn assert_turn_settings(value: &Value, model: &str, effort: &str) {
        assert_eq!(
            value.pointer("/params/model").and_then(Value::as_str),
            Some(model)
        );
        assert_eq!(
            value.pointer("/params/effort").and_then(Value::as_str),
            Some(effort)
        );
    }

    fn assert_accepts_eud_tools_mcp_approval(reply: &Value, expected_id: &Value) {
        assert_eq!(reply.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(reply.get("id"), Some(expected_id));
        assert_eq!(
            reply.pointer("/result/action").and_then(Value::as_str),
            Some("accept")
        );
        assert_eq!(reply.pointer("/result/content"), Some(&Value::Null));
    }

    fn assert_accepts_command_approval(reply: &Value, expected_id: &Value) {
        assert_eq!(reply.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(reply.get("id"), Some(expected_id));
        assert_eq!(
            reply.pointer("/result/decision").and_then(Value::as_str),
            Some("accept")
        );
    }

    fn assert_declines_approval(reply: &Value, expected_id: &Value) {
        assert_eq!(reply.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(reply.get("id"), Some(expected_id));
        assert_eq!(
            reply.pointer("/result/decision").and_then(Value::as_str),
            Some("decline")
        );
    }

    async fn next_event(
        events: &mut tokio::sync::mpsc::Receiver<AppServerEvent>,
    ) -> AppServerEvent {
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("timed out waiting for app-server event")
            .expect("app-server event channel closed")
    }

    #[tokio::test]
    async fn manual_compaction_uses_native_thread_request_and_events() {
        let (client_write, server_read) = tokio::io::duplex(16 * 1024);
        let (server_write, client_read) = tokio::io::duplex(16 * 1024);

        let stub = tokio::spawn(async move {
            let mut client_requests = BufReader::new(server_read).lines();
            let mut server_responses = server_write;

            let initialize = read_json_line(&mut client_requests).await;
            let initialize_id = assert_client_request(&initialize, "initialize");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":initialize_id,"result":{"protocolVersion":1}}),
            )
            .await;
            let initialized = read_json_line(&mut client_requests).await;
            assert_initialized_notification(&initialized);

            let compact = read_json_line(&mut client_requests).await;
            let compact_id = assert_client_request(&compact, "thread/compact/start");
            assert_eq!(
                compact.pointer("/params/threadId").and_then(Value::as_str),
                Some("thread-compact")
            );
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":compact_id,"result":{}}),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/started",
                    "params":{"threadId":"thread-compact","item":{"id":"compact-1","type":"contextCompaction"}}
                }),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/completed",
                    "params":{"threadId":"thread-compact","item":{"id":"compact-1","type":"contextCompaction"}}
                }),
            )
            .await;
        });

        let (mut client, mut events) =
            CodexAppServerClient::new_with_stdio(client_read, client_write);
        client.set_thread_id("thread-compact".to_string()).await;
        client
            .start_compaction()
            .await
            .expect("native compaction request should start");
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ContextCompactionStarted
        );
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ContextCompactionCompleted
        );
        stub.await.expect("app-server stub should complete");
    }

    #[tokio::test]
    async fn model_list_collects_all_visible_pages_in_server_order() {
        let (client_write, server_read) = tokio::io::duplex(16 * 1024);
        let (server_write, client_read) = tokio::io::duplex(16 * 1024);

        let stub = tokio::spawn(async move {
            let mut client_requests = BufReader::new(server_read).lines();
            let mut server_responses = server_write;

            let initialize = read_json_line(&mut client_requests).await;
            let initialize_id = assert_client_request(&initialize, "initialize");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":initialize_id,"result":{"protocolVersion":1}}),
            )
            .await;
            let initialized = read_json_line(&mut client_requests).await;
            assert_initialized_notification(&initialized);

            let first = read_json_line(&mut client_requests).await;
            let first_id = assert_client_request(&first, "model/list");
            assert_eq!(first.pointer("/params/includeHidden"), Some(&json!(false)));
            assert_eq!(first.pointer("/params/cursor"), Some(&Value::Null));
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":first_id,
                    "result":{
                        "data":[{
                            "model":"gpt-5.5-codex",
                            "displayName":"GPT-5.5 Codex",
                            "description":"Most capable",
                            "supportedReasoningEfforts":[{
                                "reasoningEffort":"medium",
                                "description":"Balanced"
                            }],
                            "defaultReasoningEffort":"medium",
                            "isDefault":true
                        }],
                        "nextCursor":"page-2"
                    }
                }),
            )
            .await;

            let second = read_json_line(&mut client_requests).await;
            let second_id = assert_client_request(&second, "model/list");
            assert_eq!(second.pointer("/params/cursor"), Some(&json!("page-2")));
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":second_id,
                    "result":{
                        "data":[{
                            "model":"gpt-5.4-mini",
                            "displayName":"GPT-5.4 Mini",
                            "description":"Fast",
                            "supportedReasoningEfforts":[{
                                "reasoningEffort":"low",
                                "description":"Fast"
                            }],
                            "defaultReasoningEffort":"low",
                            "isDefault":false
                        }],
                        "nextCursor":null
                    }
                }),
            )
            .await;
        });

        let (mut client, _events) = CodexAppServerClient::new_with_stdio(client_read, client_write);
        let models = client
            .list_models()
            .await
            .expect("model/list should collect every page");
        assert_eq!(
            models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-5.5-codex", "gpt-5.4-mini"]
        );
        assert_eq!(
            models[0].supported_reasoning_efforts[0].reasoning_effort,
            "medium"
        );
        stub.await.expect("stub server task should not panic");
    }

    #[tokio::test]
    async fn eud_ask_elicitation_waits_for_panel_response_without_closing() {
        let runtime = crate::tool_exec::SessionToolRuntime::for_tests();
        runtime.begin_request("req-ask", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
        });

        let (client_write, server_read) = tokio::io::duplex(16 * 1024);
        let (mut server_write, client_read) = tokio::io::duplex(16 * 1024);
        let (_client, _events) = CodexAppServerClient::new_with_stdio_and_ask_runtime(
            client_read,
            client_write,
            Some(runtime.clone()),
        );
        let mut client_requests = BufReader::new(server_read).lines();

        write_json_line(
            &mut server_write,
            json!({
                "jsonrpc": "2.0",
                "id": "ask-elicitation",
                "method": "mcpServer/elicitation/request",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "serverName": "eud-tools",
                    "mode": "form",
                    "_meta": {
                        "eudAgentAsk": {
                            "questions": [{
                                "id": "mode",
                                "question": "방식을 고르세요.",
                                "options": [{"label": "A"}, {"label": "B"}],
                                "multi": false
                            }]
                        }
                    },
                    "message": "eud-agent structured ASK",
                    "requestedSchema": {
                        "type": "object",
                        "properties": {"payload": {"type": "string"}},
                        "required": ["payload"]
                    }
                }
            }),
        )
        .await;

        let event = emitted
            .recv()
            .await
            .expect("ASK must reach the panel runtime");
        assert!(*runtime.subscribe_ask_waiting().borrow());
        runtime
            .answer_ask(
                &event.request_id,
                std::collections::BTreeMap::from([(
                    "mode".to_string(),
                    crate::ipc::AskAnswer {
                        answers: vec!["A".to_string()],
                    },
                )]),
            )
            .unwrap();

        let response = read_json_line(&mut client_requests).await;
        assert_eq!(response["id"], json!("ask-elicitation"));
        assert_eq!(response["result"]["action"], json!("accept"));
        let payload = response
            .pointer("/result/content/payload")
            .and_then(Value::as_str)
            .expect("accepted ASK response must include the encoded answers");
        let answers: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(answers["answers"]["mode"]["answers"], json!(["A"]));
        assert!(!*runtime.subscribe_ask_waiting().borrow());
    }

    #[tokio::test]
    async fn stdout_eof_marks_transport_closed_for_next_request_recovery() {
        let (client_write, server_read) = tokio::io::duplex(16 * 1024);
        let (server_write, client_read) = tokio::io::duplex(16 * 1024);

        let stub = tokio::spawn(async move {
            let mut client_requests = BufReader::new(server_read).lines();
            let mut server_responses = server_write;

            let initialize = read_json_line(&mut client_requests).await;
            let initialize_id = assert_client_request(&initialize, "initialize");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":initialize_id,"result":{"protocolVersion":1}}),
            )
            .await;
            assert_initialized_notification(&read_json_line(&mut client_requests).await);

            let model_list = read_json_line(&mut client_requests).await;
            assert_client_request(&model_list, "model/list");
        });

        let (mut client, mut events) =
            CodexAppServerClient::new_with_stdio(client_read, client_write);
        let error = client
            .list_models()
            .await
            .expect_err("stdout EOF must fail the pending request");
        assert!(error.message.contains("app-server stdout closed"));
        assert!(error.message.contains("restart automatically"));
        assert!(client.is_transport_closed());
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::Error(error.message)
        );
        stub.await.expect("stub server task should not panic");
    }

    #[tokio::test]
    async fn app_server_json_rpc_stdio_streaming_thread_reuse_and_approvals() {
        let (client_write, server_read) = tokio::io::duplex(32 * 1024);
        let (server_write, client_read) = tokio::io::duplex(32 * 1024);

        let stub = tokio::spawn(async move {
            let mut client_requests = BufReader::new(server_read).lines();
            let mut server_responses = server_write;

            let initialize = read_json_line(&mut client_requests).await;
            let initialize_id = assert_client_request(&initialize, "initialize");
            assert_initialize_params(&initialize);
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":initialize_id,"result":{"protocolVersion":1}}),
            )
            .await;
            let initialized = read_json_line(&mut client_requests).await;
            assert_initialized_notification(&initialized);

            let thread_start = read_json_line(&mut client_requests).await;
            let thread_start_id = assert_client_request(&thread_start, "thread/start");
            assert_thread_start_sandbox(&thread_start);
            assert_eud_tools_config(&thread_start, "http://127.0.0.1:54321/mcp");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":thread_start_id,"result":{}}),
            )
            .await;

            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"thread/started",
                    "params":{"thread":{"id":"thread-123"}}
                }),
            )
            .await;

            let turn_start = read_json_line(&mut client_requests).await;
            let turn_start_id = assert_client_request(&turn_start, "turn/start");
            assert_prompt(&turn_start, "first prompt");
            assert_local_image(&turn_start, "C:/tmp/screenshot.png");
            assert_turn_thread_id(&turn_start, "thread-123");
            assert_turn_settings(&turn_start, "gpt-5.5-codex", "high");
            assert_eq!(
                turn_start.pointer("/params/outputSchema/type"),
                Some(&json!("object"))
            );
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":turn_start_id,"result":{}}),
            )
            .await;

            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":"approval-mcp",
                    "method":"mcpServer/elicitation/request",
                    "params":{
                        "server":"eud-tools",
                        "serverName":"eud-tools",
                        "_meta":{"codex_approval_kind":"mcp_tool_call"},
                        "message":"Allow eud-tools MCP call?"
                    }
                }),
            )
            .await;
            let mcp_reply = read_json_line(&mut client_requests).await;
            assert_accepts_eud_tools_mcp_approval(&mcp_reply, &json!("approval-mcp"));

            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":"approval-command",
                    "method":"item/commandExecution/requestApproval",
                    "params":{"command":"cargo test"}
                }),
            )
            .await;
            let command_reply = read_json_line(&mut client_requests).await;
            assert_accepts_command_approval(&command_reply, &json!("approval-command"));

            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":"approval-file-change",
                    "method":"item/fileChange/requestApproval",
                    "params":{"changes":[{"path":"src-tauri/src/codex_client.rs"}]}
                }),
            )
            .await;
            let file_change_reply = read_json_line(&mut client_requests).await;
            assert_declines_approval(&file_change_reply, &json!("approval-file-change"));

            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":"approval-permissions",
                    "method":"item/permissions/requestApproval",
                    "params":{"reason":"read outside the session workspace"}
                }),
            )
            .await;
            let permissions_reply = read_json_line(&mut client_requests).await;
            assert_declines_approval(&permissions_reply, &json!("approval-permissions"));

            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","method":"turn/started","params":{"turnId":"turn-1"}}),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/agentMessage/delta",
                    "params":{"delta":"hello "}
                }),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/reasoning/summaryTextDelta",
                    "params":{"delta":"summary "}
                }),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/reasoning/textDelta",
                    "params":{"delta":"detail"}
                }),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/started",
                    "params":{"item":{
                        "id":"item_1",
                        "type":"mcpToolCall",
                        "server":"eud-tools",
                        "tool":"search_docs",
                        "arguments":{"query":"countdown"},
                        "status":"inProgress"
                    }}
                }),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"item/completed",
                    "params":{"item":{
                        "id":"item_1",
                        "type":"mcpToolCall",
                        "server":"eud-tools",
                        "tool":"search_docs",
                        "status":"completed",
                        "result":{"content":[{"type":"text","text":"2 hits"}]}
                    }}
                }),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","method":"turn/completed","params":{"turnId":"turn-1"}}),
            )
            .await;

            let thread_resume = read_json_line(&mut client_requests).await;
            let thread_resume_id = assert_client_request(&thread_resume, "thread/resume");
            assert_thread_id(&thread_resume, "thread-123");
            assert_eud_tools_config(&thread_resume, "http://127.0.0.1:54321/mcp");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":thread_resume_id,"result":{}}),
            )
            .await;

            let second_turn_start = read_json_line(&mut client_requests).await;
            let second_turn_start_id = assert_client_request(&second_turn_start, "turn/start");
            assert_prompt(&second_turn_start, "second prompt");
            assert_turn_thread_id(&second_turn_start, "thread-123");
            assert_turn_settings(&second_turn_start, "gpt-5.5-codex", "high");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":second_turn_start_id,"result":{}}),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","method":"turn/completed","params":{"turnId":"turn-2"}}),
            )
            .await;
        });

        let (mut client, mut events) =
            CodexAppServerClient::new_with_stdio(client_read, client_write);
        client.mcp_server_url = Some("http://127.0.0.1:54321/mcp".to_string());
        client.set_model_selection(Some(CodexModelSelection {
            model: "gpt-5.5-codex".to_string(),
            reasoning_effort: "high".to_string(),
        }));

        client
            .run_turn(CodexTurnInput {
                text: "first prompt".to_string(),
                image_paths: vec![PathBuf::from("C:/tmp/screenshot.png")],
                workspace_root: None,
                workspace_access: WorkspaceAccess::Read,
                output_schema: Some(json!({"type": "object"})),
                forbid_tools: false,
            })
            .await
            .expect("first app-server turn should complete");
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ThreadStarted {
                thread_id: "thread-123".to_string()
            }
        );
        assert_eq!(next_event(&mut events).await, AppServerEvent::TurnStarted);
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::AnswerDelta("hello ".to_string())
        );
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ReasoningDelta("summary ".to_string())
        );
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ReasoningDelta("detail".to_string())
        );
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ToolCallStarted {
                name: "search_docs".to_string(),
                args: Some("{\"query\":\"countdown\"}".to_string()),
            }
        );
        assert_eq!(
            next_event(&mut events).await,
            AppServerEvent::ToolCallCompleted {
                name: "search_docs".to_string(),
                result: Some("2 hits".to_string()),
                status: Some("completed".to_string()),
            }
        );
        assert_eq!(next_event(&mut events).await, AppServerEvent::TurnComplete);

        client
            .run_turn(CodexTurnInput::text("second prompt"))
            .await
            .expect("second app-server turn should complete");
        assert_eq!(next_event(&mut events).await, AppServerEvent::TurnComplete);

        stub.await.expect("stub server task should not panic");
    }

    #[tokio::test]
    async fn cancellable_turn_sends_interrupt_and_waits_for_completion() {
        let (client_write, server_read) = tokio::io::duplex(16 * 1024);
        let (server_write, client_read) = tokio::io::duplex(16 * 1024);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();

        let stub = tokio::spawn(async move {
            let mut client_requests = BufReader::new(server_read).lines();
            let mut server_responses = server_write;

            let initialize = read_json_line(&mut client_requests).await;
            let initialize_id = assert_client_request(&initialize, "initialize");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":initialize_id,"result":{"protocolVersion":1}}),
            )
            .await;
            assert_initialized_notification(&read_json_line(&mut client_requests).await);

            let thread_start = read_json_line(&mut client_requests).await;
            let thread_start_id = assert_client_request(&thread_start, "thread/start");
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":thread_start_id,"result":{}}),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"thread/started",
                    "params":{"thread":{"id":"thread-cancel"}}
                }),
            )
            .await;

            let turn_start = read_json_line(&mut client_requests).await;
            let turn_start_id = assert_client_request(&turn_start, "turn/start");
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "id":turn_start_id,
                    "result":{"turn":{"id":"turn-cancel"}}
                }),
            )
            .await;
            started_tx.send(()).expect("test receiver should remain");

            let interrupt = read_json_line(&mut client_requests).await;
            let interrupt_id = assert_client_request(&interrupt, "turn/interrupt");
            assert_eq!(
                interrupt
                    .pointer("/params/threadId")
                    .and_then(Value::as_str),
                Some("thread-cancel")
            );
            assert_eq!(
                interrupt.pointer("/params/turnId").and_then(Value::as_str),
                Some("turn-cancel")
            );
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":interrupt_id,"result":{}}),
            )
            .await;
            write_json_line(
                &mut server_responses,
                json!({
                    "jsonrpc":"2.0",
                    "method":"turn/completed",
                    "params":{"turn":{"id":"turn-cancel","status":"interrupted"}}
                }),
            )
            .await;
        });

        let (mut client, _events) = CodexAppServerClient::new_with_stdio(client_read, client_write);
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(0_u64);
        let run = tokio::spawn(async move {
            client
                .run_turn_cancellable(CodexTurnInput::text("cancel this turn"), cancel_rx, 0)
                .await
        });

        started_rx.await.expect("turn/start should resolve");
        cancel_tx.send(1).expect("turn should still be running");
        assert!(run
            .await
            .expect("client task should not panic")
            .expect("interrupt should complete"));
        stub.await.expect("stub server task should not panic");
    }
}
