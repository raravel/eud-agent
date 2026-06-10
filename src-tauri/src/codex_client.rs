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

use std::{env, io::ErrorKind, path::PathBuf, process::Stdio, time::Duration};

use thiserror::Error;
use tokio::{io::AsyncWriteExt, process::Command, time};

const SYSTEM_PROMPT: &str =
    "너는 스타크래프트 EUD 맵 제작용 epScript(eps) 코드를 작성하는 어시스턴트다. \
아래 [참고자료]는 네이버 카페/공식 매뉴얼에서 검색한 eps/eud3 지식이다. \
사용자 요청을 만족하는 epScript 코드만 출력해라. 설명/마크다운 없이 코드만. \
플레이어 루프·변수 선언 등 eps 관례를 지켜라.";

const RAW_SNIPPET_LIMIT: usize = 500;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodexError {
    #[error("codex command not found: {0}")]
    NotFound(String),
    #[error("codex exec timed out: {0}")]
    Timeout(String),
    #[error("codex produced no fenced code block: {0}")]
    NoCode(String),
}

/// Resolve the codex `.cmd` shim path.
///
/// Honors a `CODEX_CMD` env override (full path to the shim); otherwise locates `codex`
/// on PATH via the `which` crate. This never returns a bare `"codex"` command.
pub fn resolve_codex_cmd() -> Result<PathBuf, CodexError> {
    if let Some(cmd) = env::var_os("CODEX_CMD").filter(|cmd| !cmd.is_empty()) {
        return Ok(PathBuf::from(cmd));
    }

    which::which("codex").map_err(|err| {
        CodexError::NotFound(format!(
            "could not resolve codex.cmd via PATH: {err}. Install codex or set CODEX_CMD to the codex.cmd shim path."
        ))
    })
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

type AppServerRequestResult = Result<serde_json::Value, AppServerError>;
type AppServerPending = std::sync::Arc<
    tokio::sync::Mutex<
        std::collections::HashMap<u64, tokio::sync::oneshot::Sender<AppServerRequestResult>>,
    >,
>;
type AppServerWriter<W> = std::sync::Arc<tokio::sync::Mutex<W>>;

pub struct CodexAppServerClient<R, W> {
    _reader: std::marker::PhantomData<R>,
    writer: AppServerWriter<W>,
    pending: AppServerPending,
    next_id: u64,
    initialized: bool,
    thread_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    thread_started: std::sync::Arc<tokio::sync::Notify>,
    turn_completed: tokio::sync::broadcast::Sender<()>,
    _child: Option<tokio::process::Child>,
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
        let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
        let pending =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let thread_id = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let thread_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(128);
        let (turn_completed, _) = tokio::sync::broadcast::channel(16);

        tokio::spawn(read_app_server_stdout(
            reader,
            std::sync::Arc::clone(&writer),
            std::sync::Arc::clone(&pending),
            events_tx,
            std::sync::Arc::clone(&thread_id),
            std::sync::Arc::clone(&thread_started),
            turn_completed.clone(),
        ));

        (
            Self {
                _reader: std::marker::PhantomData,
                writer,
                pending,
                next_id: 1,
                initialized: false,
                thread_id,
                thread_started,
                turn_completed,
                _child: None,
            },
            events_rx,
        )
    }

    pub async fn run_turn(&mut self, prompt: String) -> Result<(), AppServerError> {
        if !self.initialized {
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
            self.initialized = true;
        }

        let mut turn_completed = self.turn_completed.subscribe();

        let thread_id = match self.current_thread_id().await {
            Some(thread_id) => {
                self.send_request(
                    "thread/resume",
                    serde_json::json!({ "threadId": thread_id.clone() }),
                )
                .await?;
                thread_id
            }
            None => {
                self.send_request(
                    "thread/start",
                    serde_json::json!({ "approvalPolicy": "on-request" }),
                )
                .await?;
                self.await_thread_started().await?
            }
        };

        self.send_request(
            "turn/start",
            serde_json::json!({
                "threadId": thread_id,
                "input": [{
                    "type": "text",
                    "text": prompt,
                    "text_elements": [],
                }],
            }),
        )
        .await?;

        turn_completed
            .recv()
            .await
            .map_err(|err| AppServerError::new(format!("turn completion wait failed: {err}")))?;
        Ok(())
    }

    async fn current_thread_id(&self) -> Option<String> {
        self.thread_id.lock().await.clone()
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

        if let Err(err) = write_json_rpc_line(&self.writer, request).await {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }

        rx.await
            .map_err(|err| AppServerError::new(format!("response channel closed: {err}")))?
    }
}

impl CodexAppServerClient<tokio::process::ChildStdout, tokio::process::ChildStdin> {
    pub async fn spawn_app_server(
        cwd: impl AsRef<std::path::Path>,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<AppServerEvent>), AppServerError> {
        let codex_cmd = resolve_codex_cmd().map_err(|err| AppServerError::new(err.to_string()))?;
        let mut command = tokio::process::Command::new(codex_cmd);
        command
            .arg("app-server")
            .arg("-c")
            .arg("skills.include_instructions=false")
            .arg("-c")
            .arg("model_supports_reasoning_summaries=true")
            .arg("-c")
            .arg("model_reasoning_summary=\"detailed\"")
            .current_dir(cwd.as_ref())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

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

        let (mut client, events) = Self::new_with_stdio(stdout, stdin);
        client._child = Some(child);
        Ok((client, events))
    }
}

async fn read_app_server_stdout<R, W>(
    reader: R,
    writer: AppServerWriter<W>,
    pending: AppServerPending,
    events_tx: tokio::sync::mpsc::Sender<AppServerEvent>,
    thread_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    thread_started: std::sync::Arc<tokio::sync::Notify>,
    turn_completed: tokio::sync::broadcast::Sender<()>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::AsyncBufReadExt as _;

    let mut lines = tokio::io::BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(err) => {
                let _ = events_tx
                    .send(AppServerEvent::Error(format!(
                        "failed reading app-server stdout: {err}"
                    )))
                    .await;
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
                    break;
                }
                continue;
            }
        };

        let method = message.get("method").and_then(serde_json::Value::as_str);
        let id = message.get("id").cloned();

        match (method, id) {
            (Some(method), Some(id)) => {
                if handle_server_request(&writer, method, id, message.get("params"))
                    .await
                    .is_err()
                {
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
                    break;
                }
            }
            (None, Some(id)) => {
                complete_pending_request(&pending, id, &message).await;
            }
            (None, None) => {}
        }
    }

    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(AppServerError::new("app-server stdout closed")));
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
) -> Result<(), AppServerError>
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let result = if should_accept_mcp_elicitation(method, params) {
        serde_json::json!({ "action": "accept", "content": null })
    } else {
        decline_approval_result(method)
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
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/permissions/requestApproval" => serde_json::json!({ "decision": "decline" }),
        "execCommandApproval" | "applyPatchApproval" => {
            serde_json::json!({ "decision": "denied" })
        }
        _ => serde_json::json!({ "decision": "decline" }),
    }
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

    ["server", "serverName", "server_name", "name"]
        .iter()
        .any(|key| params.get(*key).and_then(serde_json::Value::as_str) == Some("eud-tools"))
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
            let event = tool_event_from_item(params, false).unwrap_or_else(|| {
                AppServerEvent::ItemStarted {
                    item_id: string_param(params, &["itemId", "item_id", "id"]),
                }
            });
            send_event(events_tx, event).await
        }
        "item/completed" => {
            let event = tool_event_from_item(params, true).unwrap_or_else(|| {
                AppServerEvent::ItemCompleted {
                    item_id: string_param(params, &["itemId", "item_id", "id"]),
                }
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
mod appserver_tests {
    use super::{AppServerEvent, CodexAppServerClient};
    use serde_json::{json, Value};
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

    fn assert_initialize_params(value: &Value) {
        assert_eq!(
            value
                .pointer("/params/clientInfo/name")
                .and_then(Value::as_str),
            Some("eud-agent")
        );
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

    fn assert_accepts_eud_tools_mcp_approval(reply: &Value, expected_id: &Value) {
        assert_eq!(reply.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(reply.get("id"), Some(expected_id));
        assert_eq!(
            reply.pointer("/result/action").and_then(Value::as_str),
            Some("accept")
        );
        assert_eq!(reply.pointer("/result/content"), Some(&Value::Null));
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
                    "params":{"thread":{"id":"thread-123"}}
                }),
            )
            .await;

            let turn_start = read_json_line(&mut client_requests).await;
            let turn_start_id = assert_client_request(&turn_start, "turn/start");
            assert_prompt(&turn_start, "first prompt");
            assert_turn_thread_id(&turn_start, "thread-123");
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
            assert_declines_approval(&command_reply, &json!("approval-command"));

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
            write_json_line(
                &mut server_responses,
                json!({"jsonrpc":"2.0","id":thread_resume_id,"result":{}}),
            )
            .await;

            let second_turn_start = read_json_line(&mut client_requests).await;
            let second_turn_start_id = assert_client_request(&second_turn_start, "turn/start");
            assert_prompt(&second_turn_start, "second prompt");
            assert_turn_thread_id(&second_turn_start, "thread-123");
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

        client
            .run_turn("first prompt".to_string())
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
            .run_turn("second prompt".to_string())
            .await
            .expect("second app-server turn should complete");
        assert_eq!(next_event(&mut events).await, AppServerEvent::TurnComplete);

        stub.await.expect("stub server task should not panic");
    }
}
