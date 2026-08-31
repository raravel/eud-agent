//! Official Claude Code CLI print-mode driver with isolated profile and eud-tools-only MCP.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use base64::Engine as _;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

use crate::claude_auth::resolve_claude_cmd;
use crate::codex_client::{AgentTurnInput, WorkspaceAccess};
use crate::engine::{
    AgentDriver, AgentEngineError, AgentTurnResult, EngineEvent, EventSink, SessionEventSink,
};
use crate::provider::{
    ModelCapabilities, ProviderConversationState, ProviderId, ProviderModel, ReasoningSelection,
};
use crate::provider_tool_loop::validate_structured_output;
use crate::tool_exec::SessionToolRuntime;
use crate::workspace::{PreparedWorkspace, WorkspaceManager, WorkspaceTurnRecorder};

const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;
const MAX_STDOUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 16 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const CLAUDE_PROVIDER_DEFAULT: &str = "provider-default";

pub fn provider_managed_models(selected: Option<&str>) -> Vec<ProviderModel> {
    vec![ProviderModel {
        provider: ProviderId::ClaudeCode,
        model: CLAUDE_PROVIDER_DEFAULT.to_string(),
        display_name: "Claude Code 기본 모델".to_string(),
        description: "Claude Code가 현재 계정과 배포 기준으로 모델을 선택합니다.".to_string(),
        is_default: selected == Some(CLAUDE_PROVIDER_DEFAULT),
        capabilities: ModelCapabilities {
            vision: true,
            tool_calls: true,
            strict_structured_output: true,
            reasoning_levels: Vec::new(),
            native_compaction: true,
            context_window: None,
            hosted_web_search: false,
        },
        privacy: None,
    }]
}

pub struct ProductionClaudeCodeDriver {
    session_id: String,
    model: String,
    conversation_id: Option<String>,
    dirs: crate::config::DataDirs,
    sink: SessionEventSink,
    mcp_port: Option<u16>,
    runtime: SessionToolRuntime,
    workspace: WorkspaceManager,
    active_workspace: Option<PreparedWorkspace>,
    workspace_override: Option<PreparedWorkspace>,
    cancellation: tokio::sync::watch::Receiver<u64>,
    persist_context_usage: bool,
}

impl ProductionClaudeCodeDriver {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session_id: String,
        model: String,
        _reasoning: Option<ReasoningSelection>,
        dirs: crate::config::DataDirs,
        sink: SessionEventSink,
        mcp_port: Option<u16>,
        runtime: SessionToolRuntime,
        cancellation: tokio::sync::watch::Receiver<u64>,
    ) -> Result<Self, AgentEngineError> {
        if model != CLAUDE_PROVIDER_DEFAULT {
            return Err(AgentEngineError::new("provider_model_unavailable"));
        }
        Ok(Self {
            session_id,
            model,
            conversation_id: None,
            workspace: WorkspaceManager::new(dirs.clone()),
            dirs,
            sink,
            mcp_port,
            runtime,
            active_workspace: None,
            workspace_override: None,
            cancellation,
            persist_context_usage: true,
        })
    }

    pub(crate) fn use_workspace(&mut self, workspace: PreparedWorkspace) {
        self.workspace_override = Some(workspace);
    }

    pub(crate) fn disable_session_persistence(&mut self) {
        self.persist_context_usage = false;
    }

    async fn prepare_workspace(
        &mut self,
        access: WorkspaceAccess,
    ) -> Result<(PreparedWorkspace, Option<WorkspaceTurnRecorder>), AgentEngineError> {
        let request_id = self.runtime.current_request_id().ok_or_else(|| {
            AgentEngineError::new("no request is open for the provider workspace")
        })?;
        if access == WorkspaceAccess::Write && !self.runtime.owns_write_registration() {
            return Err(AgentEngineError::new(
                "write-mode provider execution requires an active workspace write registration",
            ));
        }
        let manager = self.workspace.clone();
        let override_workspace = self.workspace_override.clone();
        let session_id = self.session_id.clone();
        let baseline_request = request_id.clone();
        let (workspace, baseline) = tokio::task::spawn_blocking(move || {
            let workspace = match override_workspace {
                Some(workspace) => workspace,
                None => manager.prepare_session_current(&session_id)?,
            };
            let baseline = if access == WorkspaceAccess::Write {
                Some(
                    manager
                        .begin_turn(&workspace, &baseline_request)
                        .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            Ok::<_, String>((workspace, baseline))
        })
        .await
        .map_err(|error| AgentEngineError::new(error.to_string()))?
        .map_err(AgentEngineError::new)?;
        validate_workspace_boundary(&workspace.root).map_err(AgentEngineError::new)?;
        self.runtime
            .bind_workspace_root(&request_id, workspace.root.clone())
            .map_err(AgentEngineError::new)?;
        self.active_workspace = Some(workspace.clone());
        let recorder = baseline.map(|baseline| {
            WorkspaceTurnRecorder::new(
                self.workspace.clone(),
                baseline,
                self.runtime.journal().clone(),
            )
        });
        Ok((workspace, recorder))
    }

    async fn run_stream_turn(
        &mut self,
        input: AgentTurnInput,
        compaction: bool,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        self.sink
            .emit(EngineEvent::Progress(crate::ipc::ProgressEvent {
                stage: if compaction {
                    crate::ipc::ProgressStage::Compaction
                } else {
                    crate::ipc::ProgressStage::Provider
                },
                detail: Some(if compaction {
                    "Claude Code compaction started".to_string()
                } else {
                    "Claude Code turn started".to_string()
                }),
                provider: Some(ProviderId::ClaudeCode),
                model: Some(self.model.clone()),
            }))?;
        let (workspace, mut recorder) = self.prepare_workspace(input.workspace_access).await?;
        let executable = resolve_claude_cmd(
            &self.dirs,
            &self
                .dirs
                .load_config()
                .map_err(|_| AgentEngineError::new("provider_protocol_changed"))?,
        )
        .map_err(AgentEngineError::new)?;
        let mcp_config = if compaction {
            None
        } else {
            let mcp_port = self
                .mcp_port
                .ok_or_else(|| AgentEngineError::new("provider eud-tools MCP is unavailable"))?;
            Some(
                json!({
                    "mcpServers": {
                        "eud-tools": {
                            "type": "http",
                            "url": format!("http://127.0.0.1:{mcp_port}/mcp")
                        }
                    }
                })
                .to_string(),
            )
        };
        let mut command = tokio::process::Command::new(executable);
        configure_tokio_command(&mut command, &self.dirs);
        command.args(stream_args(
            mcp_config.as_deref(),
            self.conversation_id.as_deref(),
            compaction,
        ));
        command
            .current_dir(&workspace.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
        let job = WindowsJob::assign(&child)
            .map_err(|_| AgentEngineError::new("provider process isolation unavailable"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentEngineError::new("provider_transport_closed"))?;
        let message = claude_user_message(&input).map_err(AgentEngineError::new)?;
        let mut line = serde_json::to_vec(&message)
            .map_err(|_| AgentEngineError::new("provider_protocol_changed"))?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
        stdin
            .shutdown()
            .await
            .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
        drop(stdin);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentEngineError::new("provider_transport_closed"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentEngineError::new("provider_transport_closed"))?;
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.take(MAX_STDERR_BYTES).read_to_end(&mut bytes).await;
            bytes
        });
        let generation = *self.cancellation.borrow_and_update();
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let mut total_bytes = 0_usize;
        let mut parser = ClaudeStreamParser::default();
        let mut stdout_closed = false;
        loop {
            tokio::select! {
                line = lines.next_line(), if !stdout_closed => {
                    match line.map_err(|_| AgentEngineError::new("provider_transport_closed"))? {
                        Some(line) => {
                            total_bytes = total_bytes.saturating_add(line.len());
                            if line.len() > MAX_JSONL_LINE_BYTES || total_bytes > MAX_STDOUT_BYTES {
                                terminate_child(&mut child, job).await;
                                return Err(AgentEngineError::new("provider_protocol_changed"));
                            }
                            let value: Value = serde_json::from_str(&line)
                                .map_err(|_| AgentEngineError::new("provider_protocol_changed"))?;
                            parser.apply(&value)?;
                            self.emit_parser_events(&mut parser)?;
                        }
                        None => stdout_closed = true,
                    }
                }
                status = child.wait() => {
                    let status = status.map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
                    let _ = stderr_task.await;
                    drop(job);
                    if !status.success() || parser.is_error {
                        return Err(AgentEngineError::new(parser.error_code.unwrap_or_else(|| "provider_transport_closed".to_string())));
                    }
                    parser.validate_init(!compaction)?;
                    let session_id = parser.session_id.clone().ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?;
                    if let Some(existing) = self.conversation_id.as_deref() {
                        if existing != session_id {
                            return Err(AgentEngineError::new("provider_protocol_changed"));
                        }
                    } else if self.persist_context_usage {
                        self.conversation_id = Some(session_id);
                    }
                    if let Some(usage) = parser.usage.clone() {
                        self.emit_usage(usage)?;
                    }
                    if let Some(recorder) = recorder.as_mut() {
                        recorder.finish().map_err(|error| AgentEngineError::new(error.to_string()))?;
                    }
                    return Ok(AgentTurnResult::Answer { text: parser.result.unwrap_or(parser.answer) });
                }
                changed = self.cancellation.changed() => {
                    if changed.is_ok() && *self.cancellation.borrow() != generation {
                        terminate_child(&mut child, job).await;
                        let _ = stderr_task.await;
                        return Ok(AgentTurnResult::Cancelled);
                    }
                }
            }
        }
    }

    async fn run_structured_turn(
        &mut self,
        input: AgentTurnInput,
        schema: Value,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        self.sink
            .emit(EngineEvent::Progress(crate::ipc::ProgressEvent {
                stage: crate::ipc::ProgressStage::Provider,
                detail: Some("Claude Code structured turn started".to_string()),
                provider: Some(ProviderId::ClaudeCode),
                model: Some(self.model.clone()),
            }))?;
        let (workspace, _recorder) = self.prepare_workspace(input.workspace_access).await?;
        if !input.image_paths.is_empty() {
            return Err(AgentEngineError::new("provider_capability_unsupported"));
        }
        let executable = resolve_claude_cmd(
            &self.dirs,
            &self
                .dirs
                .load_config()
                .map_err(|_| AgentEngineError::new("provider_protocol_changed"))?,
        )
        .map_err(AgentEngineError::new)?;
        let mut command = tokio::process::Command::new(executable);
        configure_tokio_command(&mut command, &self.dirs);
        command
            .arg("-p")
            .arg(&input.text)
            .args(["--output-format", "json"])
            .arg("--json-schema")
            .arg(schema.to_string())
            .arg("--tools")
            .arg("")
            .arg("--no-session-persistence")
            .args(["--permission-mode", "dontAsk"])
            .arg("--disable-slash-commands")
            .arg("--no-chrome")
            .current_dir(workspace.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut command);
        let child = command
            .spawn()
            .map_err(|_| AgentEngineError::new("provider_transport_closed"))?;
        let job = WindowsJob::assign(&child)
            .map_err(|_| AgentEngineError::new("provider process isolation unavailable"))?;
        let generation = *self.cancellation.borrow_and_update();
        let output = tokio::select! {
            output = child.wait_with_output() => output.map_err(|_| AgentEngineError::new("provider_transport_closed"))?,
            changed = self.cancellation.changed() => {
                if changed.is_ok() && *self.cancellation.borrow() != generation {
                    job.terminate();
                    return Ok(AgentTurnResult::Cancelled);
                }
                return Err(AgentEngineError::new("provider_transport_closed"));
            }
        };
        drop(job);
        if !output.status.success() || output.stdout.len() > MAX_STDOUT_BYTES {
            return Err(AgentEngineError::new("provider_structured_output_invalid"));
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| AgentEngineError::new("provider_structured_output_invalid"))?;
        let structured = value
            .get("structured_output")
            .cloned()
            .ok_or_else(|| AgentEngineError::new("provider_structured_output_invalid"))?;
        validate_structured_output(&schema, &structured).map_err(AgentEngineError::new)?;
        let text = serde_json::to_string(&structured)
            .map_err(|_| AgentEngineError::new("provider_structured_output_invalid"))?;
        Ok(AgentTurnResult::Answer { text })
    }

    fn emit_parser_events(&self, parser: &mut ClaudeStreamParser) -> Result<(), AgentEngineError> {
        if !parser.pending_reasoning.is_empty() {
            let detail = std::mem::take(&mut parser.pending_reasoning);
            self.sink.emit(EngineEvent::Agent(crate::ipc::AgentEvent {
                kind: "reasoning_delta".to_string(),
                detail,
                data: None,
            }))?;
        }
        if !parser.pending_text.is_empty() {
            let detail = std::mem::take(&mut parser.pending_text);
            self.sink.emit(EngineEvent::Agent(crate::ipc::AgentEvent {
                kind: "message_delta".to_string(),
                detail,
                data: None,
            }))?;
        }
        Ok(())
    }

    fn emit_usage(&self, usage: crate::ipc::ContextUsage) -> Result<(), AgentEngineError> {
        if !self.persist_context_usage {
            return Ok(());
        }
        self.sink
            .emit(EngineEvent::ContextUsage(crate::ipc::ContextUsageEvent {
                turn_id: uuid::Uuid::new_v4().to_string(),
                token_usage: usage,
            }))
    }
}

impl AgentDriver for ProductionClaudeCodeDriver {
    async fn run_turn(
        &mut self,
        input: AgentTurnInput,
    ) -> Result<AgentTurnResult, AgentEngineError> {
        if input.forbid_tools {
            if let Some(schema) = input.output_schema.clone() {
                return self.run_structured_turn(input, schema).await;
            }
        }
        self.run_stream_turn(input, false).await
    }

    async fn compile_task_state(
        &mut self,
        input: AgentTurnInput,
    ) -> Result<Option<String>, AgentEngineError> {
        match self.run_turn(input).await? {
            AgentTurnResult::Answer { text } => Ok(Some(text)),
            AgentTurnResult::Cancelled => Err(AgentEngineError::new("provider_cancelled")),
            AgentTurnResult::Plan { .. } => {
                Err(AgentEngineError::new("provider_structured_output_invalid"))
            }
        }
    }

    async fn compact_conversation(&mut self) -> Result<(), AgentEngineError> {
        if self.conversation_id.is_none() {
            return Err(AgentEngineError::new("provider conversation is empty"));
        }
        let result = self
            .run_stream_turn(AgentTurnInput::text("/compact").without_tools(), true)
            .await?;
        if matches!(result, AgentTurnResult::Cancelled) {
            return Err(AgentEngineError::new("provider_cancelled"));
        }
        Ok(())
    }

    async fn reset_conversation(&mut self) -> Result<(), AgentEngineError> {
        self.conversation_id = None;
        Ok(())
    }

    async fn conversation_state(&self) -> ProviderConversationState {
        ProviderConversationState::ClaudeCode {
            session_id: self.conversation_id.clone(),
        }
    }

    async fn seed_conversation(
        &mut self,
        state: ProviderConversationState,
    ) -> Result<(), AgentEngineError> {
        let ProviderConversationState::ClaudeCode { session_id } = state else {
            return Err(AgentEngineError::new(
                "Claude Code driver received incompatible conversation state",
            ));
        };
        self.conversation_id = session_id;
        Ok(())
    }

    fn current_workspace(&self) -> Option<PreparedWorkspace> {
        self.active_workspace.clone()
    }
}

fn claude_user_message(input: &AgentTurnInput) -> Result<Value, String> {
    let mut content = vec![json!({"type":"text","text":input.text})];
    for path in &input.image_paths {
        let (mime, data) = encoded_image(path)?;
        content.push(json!({
            "type": "image",
            "source": {"type":"base64","media_type":mime,"data":data}
        }));
    }
    Ok(json!({
        "type": "user",
        "message": {"role":"user","content":content},
        "parent_tool_use_id": Value::Null
    }))
}

fn encoded_image(path: &Path) -> Result<(&'static str, String), String> {
    let bytes = std::fs::read(path).map_err(|_| "provider image cannot be read".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err("provider image size is unsupported".to_string());
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        return Err("provider image format is unsupported".to_string());
    };
    Ok((
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

fn validate_workspace_boundary(root: &Path) -> Result<(), String> {
    for relative in [".claude", ".mcp.json", "CLAUDE.md", "CLAUDE.local.md"] {
        if root.join(relative).exists() {
            return Err("provider workspace contains ambient Claude configuration".to_string());
        }
    }
    Ok(())
}

fn stream_args(
    mcp_config: Option<&str>,
    conversation_id: Option<&str>,
    compaction: bool,
) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];
    if compaction {
        args.extend([
            "--strict-mcp-config".to_string(),
            "--tools".to_string(),
            String::new(),
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--no-chrome".to_string(),
        ]);
    } else if let Some(mcp_config) = mcp_config {
        args.extend([
            "--mcp-config".to_string(),
            mcp_config.to_string(),
            "--strict-mcp-config".to_string(),
            "--tools".to_string(),
            String::new(),
            "--allowedTools".to_string(),
            "mcp__eud-tools__*".to_string(),
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--disable-slash-commands".to_string(),
            "--no-chrome".to_string(),
        ]);
    }
    if let Some(conversation_id) = conversation_id {
        args.extend(["--resume".to_string(), conversation_id.to_string()]);
    }
    args
}

fn configure_tokio_command(command: &mut tokio::process::Command, dirs: &crate::config::DataDirs) {
    command.env("CLAUDE_CONFIG_DIR", dirs.claude_config_dir());
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_PROFILE",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_ORGANIZATION_ID",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_SIMPLE",
    ] {
        command.env_remove(name);
    }
}

#[cfg(windows)]
fn hide_console(command: &mut tokio::process::Command) {
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut tokio::process::Command) {}

#[cfg(windows)]
struct WindowsJob(isize);

#[cfg(windows)]
impl WindowsJob {
    fn assign(child: &tokio::process::Child) -> std::io::Result<Self> {
        use std::mem::size_of;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        let assigned = if configured != 0 {
            let child_handle = child
                .raw_handle()
                .ok_or_else(std::io::Error::last_os_error)?;
            unsafe { AssignProcessToJobObject(handle, child_handle as HANDLE) }
        } else {
            0
        };
        if configured == 0 || assigned == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self(handle as isize))
    }

    fn terminate(self) {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        unsafe { TerminateJobObject(self.0 as HANDLE, 1) };
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        unsafe { CloseHandle(self.0 as HANDLE) };
    }
}

#[cfg(not(windows))]
struct WindowsJob;

#[cfg(not(windows))]
impl WindowsJob {
    fn assign(_child: &tokio::process::Child) -> std::io::Result<Self> {
        Ok(Self)
    }
    fn terminate(self) {}
}

async fn terminate_child(child: &mut tokio::process::Child, job: WindowsJob) {
    let _ = child.start_kill();
    if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        job.terminate();
    }
}

#[derive(Default)]
struct ClaudeStreamParser {
    initialized: bool,
    tools: Vec<String>,
    mcp_ready: bool,
    session_id: Option<String>,
    answer: String,
    pending_text: String,
    pending_reasoning: String,
    result: Option<String>,
    usage: Option<crate::ipc::ContextUsage>,
    is_error: bool,
    error_code: Option<String>,
    tool_parts: BTreeMap<u64, String>,
}

impl ClaudeStreamParser {
    fn apply(&mut self, value: &Value) -> Result<(), AgentEngineError> {
        match value.get("type").and_then(Value::as_str) {
            Some("system") if value.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.initialized = true;
                self.session_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.tools = value
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?
                    .iter()
                    .map(|tool| {
                        tool.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.mcp_ready = value
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .is_some_and(|servers| {
                        servers.iter().any(|server| {
                            server.get("name").and_then(Value::as_str) == Some("eud-tools")
                                && matches!(
                                    server.get("status").and_then(Value::as_str),
                                    Some("connected" | "ready")
                                )
                        })
                    });
                if value
                    .get("mcp_server_errors")
                    .and_then(Value::as_array)
                    .is_some_and(|errors| !errors.is_empty())
                    || value
                        .get("plugin_errors")
                        .and_then(Value::as_array)
                        .is_some_and(|errors| !errors.is_empty())
                {
                    return Err(AgentEngineError::new("provider_protocol_changed"));
                }
            }
            Some("system") if value.get("subtype").and_then(Value::as_str) == Some("api_retry") => {
                self.error_code = value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(map_claude_error);
            }
            Some("stream_event") => self.apply_stream_event(
                value
                    .get("event")
                    .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?,
            )?,
            Some("result") => {
                self.session_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.session_id.clone());
                self.result = value
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.is_error = value
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(usage) = value.get("usage") {
                    self.usage = parse_claude_usage(usage);
                }
                if self.is_error {
                    self.error_code = value
                        .get("subtype")
                        .and_then(Value::as_str)
                        .map(map_claude_error)
                        .or_else(|| self.error_code.clone());
                }
            }
            Some("assistant" | "user") => {}
            Some(_) => {}
            None => return Err(AgentEngineError::new("provider_protocol_changed")),
        }
        Ok(())
    }

    fn apply_stream_event(&mut self, event: &Value) -> Result<(), AgentEngineError> {
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                if event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
                {
                    let index = event
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?;
                    self.tool_parts.insert(index, String::new());
                }
            }
            Some("content_block_delta") => {
                let delta = event
                    .get("delta")
                    .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?;
                        self.answer.push_str(text);
                        self.pending_text.push_str(text);
                    }
                    Some("thinking_delta") => {
                        let text = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?;
                        self.pending_reasoning.push_str(text);
                    }
                    Some("input_json_delta") => {
                        let index = event
                            .get("index")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| AgentEngineError::new("provider_protocol_changed"))?;
                        self.tool_parts.entry(index).or_default().push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    AgentEngineError::new("provider_protocol_changed")
                                })?,
                        );
                    }
                    _ => {}
                }
            }
            Some("message_start" | "message_delta" | "message_stop" | "content_block_stop") => {}
            Some(_) => {}
            None => return Err(AgentEngineError::new("provider_protocol_changed")),
        }
        Ok(())
    }

    fn validate_init(&self, require_mcp: bool) -> Result<(), AgentEngineError> {
        let tools_valid = if require_mcp {
            self.tools
                .iter()
                .all(|tool| tool.starts_with("mcp__eud-tools__"))
        } else {
            self.tools.is_empty()
        };
        if !self.initialized || (require_mcp && !self.mcp_ready) || !tools_valid {
            return Err(AgentEngineError::new(
                "provider process boundary validation failed",
            ));
        }
        Ok(())
    }
}

fn map_claude_error(error: &str) -> String {
    match error {
        "authentication_failed" | "oauth_org_not_allowed" => "provider_not_authenticated",
        "billing_error" => "provider_quota_exhausted",
        "rate_limit" => "provider_rate_limited",
        "model_not_found" => "provider_model_unavailable",
        "overloaded" | "server_error" => "provider_transport_closed",
        _ => "provider_protocol_changed",
    }
    .to_string()
}

fn parse_claude_usage(value: &Value) -> Option<crate::ipc::ContextUsage> {
    let input = value.get("input_tokens")?.as_i64()?;
    let output = value
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = value
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_write = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let last = crate::ipc::TokenUsageBreakdown {
        input_tokens: input,
        cached_input_tokens: cached,
        cache_write_input_tokens: cache_write,
        output_tokens: output,
        reasoning_output_tokens: 0,
        total_tokens: input.saturating_add(output),
    };
    Some(crate::ipc::ContextUsage {
        last: last.clone(),
        total: last,
        model_context_window: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_parser_requires_only_eud_tools_and_connected_mcp() {
        let mut parser = ClaudeStreamParser::default();
        parser
            .apply(&json!({
                "type":"system","subtype":"init","session_id":"session-1",
                "tools":["mcp__eud-tools__read_file"],
                "mcp_servers":[{"name":"eud-tools","status":"connected"}]
            }))
            .unwrap();
        parser.validate_init(true).unwrap();
        parser.tools.push("Read".to_string());
        assert!(parser.validate_init(true).is_err());
    }

    #[test]
    fn stream_parser_assembles_partial_text_and_reasoning() {
        let mut parser = ClaudeStreamParser::default();
        parser.apply(&json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}})).unwrap();
        parser.apply(&json!({"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"why"}}})).unwrap();
        assert_eq!(parser.answer, "hello");
        assert_eq!(parser.pending_reasoning, "why");
    }

    #[test]
    fn workspace_boundary_rejects_ambient_claude_files() {
        let root =
            std::env::temp_dir().join(format!("eud-claude-boundary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(validate_workspace_boundary(&root).is_ok());
        std::fs::write(root.join("CLAUDE.md"), "ambient").unwrap();
        assert!(validate_workspace_boundary(&root).is_err());
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    fn stream_args_disable_builtins_and_pin_exact_mcp_resume_contract() {
        let args = stream_args(
            Some(
                r#"{"mcpServers":{"eud-tools":{"type":"http","url":"http://127.0.0.1:1234/mcp"}}}"#,
            ),
            Some("session-1"),
            false,
        );
        for required in [
            "--input-format",
            "stream-json",
            "--output-format",
            "--include-partial-messages",
            "--strict-mcp-config",
            "--tools",
            "--allowedTools",
            "mcp__eud-tools__*",
            "--permission-mode",
            "dontAsk",
            "--disable-slash-commands",
            "--no-chrome",
            "--resume",
            "session-1",
        ] {
            assert!(args.iter().any(|argument| argument == required));
        }
        let tools = args
            .iter()
            .position(|argument| argument == "--tools")
            .unwrap();
        assert_eq!(args[tools + 1], "");
        assert!(!args.iter().any(|argument| {
            matches!(
                argument.as_str(),
                "--bare" | "--fallback-model" | "--model" | "--effort"
            )
        }));
    }

    #[test]
    fn catalog_exposes_only_the_cli_managed_default_behavior() {
        let models = provider_managed_models(Some(CLAUDE_PROVIDER_DEFAULT));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, CLAUDE_PROVIDER_DEFAULT);
        assert!(models[0].is_default);
        assert!(models[0].capabilities.reasoning_levels.is_empty());
    }
}
