use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

use crate::provider_runtime::{
    AdapterEvent, AdapterEventKind, NormalizedBlock, ProviderRuntimeError, RunIdentity,
};

use super::events::{send_event, ParsedEvent};
use super::foreground::NativeRunResult;
use super::parser::ClaudeStreamParser;
use super::request::{configure_tokio_command, hide_console, terminate_child, MAX_STDERR_BYTES};
use super::state::{PreparedClaudeProcess, ProductionClaudeCodeAdapter};

pub(super) struct StreamProcessRequest<'a> {
    pub(super) identity: &'a RunIdentity,
    pub(super) cwd: &'a Path,
    pub(super) args: Vec<String>,
    pub(super) message: Value,
    pub(super) require_mcp: bool,
    pub(super) max_output_bytes: usize,
    pub(super) cancellation: tokio::sync::watch::Receiver<u64>,
    pub(super) events: Option<tokio::sync::mpsc::Sender<AdapterEvent>>,
}

impl ProductionClaudeCodeAdapter {
    pub(super) fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.executable);
        command.args(&self.executable_prefix_args);
        configure_tokio_command(&mut command, &self.profile_dir);
        command
    }

    pub(super) fn spawn_stream_process(
        &self,
        cwd: &Path,
        args: Vec<String>,
    ) -> Result<PreparedClaudeProcess, ProviderRuntimeError> {
        let mut command = self.command();
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut command);
        let child = command.spawn().map_err(|_| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let job = crate::provider_process::WindowsJob::assign(&child).map_err(|_| {
            ProviderRuntimeError::Transport("provider process isolation unavailable".to_string())
        })?;
        Ok(PreparedClaudeProcess { child, job })
    }

    pub(super) async fn run_stream_process(
        &mut self,
        request: StreamProcessRequest<'_>,
    ) -> Result<NativeRunResult, ProviderRuntimeError> {
        let StreamProcessRequest {
            identity,
            cwd,
            args,
            message,
            require_mcp,
            max_output_bytes,
            mut cancellation,
            events,
        } = request;
        let PreparedClaudeProcess { mut child, job } = match self.prepared_compaction.take() {
            Some(prepared) if !require_mcp => prepared,
            Some(_) | None => self.spawn_stream_process(cwd, args)?,
        };
        if *cancellation.borrow_and_update() != identity.cancellation_generation {
            terminate_child(&mut child, job).await;
            return Ok(NativeRunResult::Cancelled);
        }
        let mut stdin = child.stdin.take().ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let mut line = serde_json::to_vec(&message)
            .map_err(|_| ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()))?;
        line.push(b'\n');
        stdin.write_all(&line).await.map_err(|_| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        stdin.shutdown().await.map_err(|_| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        drop(stdin);
        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.take(MAX_STDERR_BYTES).read_to_end(&mut bytes).await;
            bytes
        });
        let mut stdout = tokio::io::BufReader::new(stdout);
        let mut total_bytes = 0_usize;
        let mut parser = ClaudeStreamParser::default();
        let response_id = format!("claude-{}", identity.run_id.get());
        let mut started = false;
        loop {
            tokio::select! {
                read = read_bounded_jsonl_line(&mut stdout) => {
                    match read? {
                        Some((line, consumed_bytes)) => {
                            total_bytes = total_bytes.saturating_add(consumed_bytes);
                            if total_bytes > max_output_bytes {
                                terminate_child(&mut child, job).await;
                                let _ = stderr_task.await;
                                return Err(ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()));
                            }
                            let value: Value = serde_json::from_slice(&line)
                                .map_err(|_| ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()))?;
                            if value.pointer("/event/content_block/type").and_then(Value::as_str) == Some("tool_use")
                                && (!require_mcp || !value.pointer("/event/content_block/name").and_then(Value::as_str).is_some_and(super::request::tool_is_authorized))
                            {
                                return Err(ProviderRuntimeError::Protocol("provider process boundary validation failed".into()));
                            }
                            // Judge the published session before the init line is
                            // validated, so an init that also fails validation still
                            // cannot hand the resume target back as a boundary.
                            if !started
                                && value.get("type").and_then(Value::as_str) == Some("system")
                                && value.get("subtype").and_then(Value::as_str) == Some("init")
                            {
                                if let Some(expected) = &self.resume_target {
                                    self.session_rejected = value.get("session_id").and_then(Value::as_str) != Some(expected.as_str());
                                }
                            }
                            let parsed = parser.apply(&value)?;
                            if parser.initialized && !started {
                                parser.validate_init(require_mcp)?;
                                started = true;
                                // The CLI publishes its resumable session id before any
                                // output; retaining it here is what lets an interrupted
                                // run keep a native continuation boundary. A session that
                                // is not the one the run asked to resume is a protocol
                                // deviation and is never adopted as a boundary.
                                let consistent = match (&self.resume_target, &parser.session_id) {
                                    (Some(expected), Some(session)) => expected == session,
                                    (Some(_), None) => false,
                                    (None, _) => true,
                                };
                                self.session_rejected |= !consistent;
                                if consistent {
                                    self.observed_session_id.clone_from(&parser.session_id);
                                    if let Some(session_id) = parser.session_id.clone() {
                                        send_event(&events, identity, AdapterEventKind::NativeSessionStarted { session_id }).await?;
                                    }
                                }
                                send_event(&events, identity, AdapterEventKind::ResponseStarted { response_id: response_id.clone() }).await?;
                            }
                            for event in parsed {
                                if let ParsedEvent::ToolObservation { name, .. } = &event {
                                    if !require_mcp || !super::request::tool_is_authorized(name) {
                                        return Err(ProviderRuntimeError::Protocol(
                                            "provider process boundary validation failed".to_string(),
                                        ));
                                    }
                                }
                                let kind = match event {
                                    ParsedEvent::Text(text) => AdapterEventKind::Block(NormalizedBlock::Text { response_id: response_id.clone(), text }),
                                    ParsedEvent::Reasoning(text) => AdapterEventKind::Block(NormalizedBlock::Reasoning { response_id: response_id.clone(), text, continuation: None }),
                                    ParsedEvent::ToolObservation { call_id, name, arguments, result, status } => {
                                        let mcp_server = name.strip_prefix("mcp__").and_then(|name| name.split_once("__")).filter(|(server, tool)| !server.is_empty() && !tool.is_empty()).map(|(server, _)| server.to_string());
                                        AdapterEventKind::NativeToolObservation { call_id, mcp_server, name, arguments, result, status }
                                    },
                                };
                                send_event(&events, identity, kind).await?;
                            }
                        }
                        None => {
                            let status = child.wait().await.map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?;
                            let _ = stderr_task.await;
                            drop(job);
                            if !status.success() || parser.is_error {
                                return Err(ProviderRuntimeError::Transport(parser.error_code.unwrap_or_else(|| "provider_transport_closed".to_string())));
                            }
                            parser.validate_init(require_mcp)?;
                            let session_id = parser.session_id.ok_or_else(|| ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()))?;
                            if parser.result.is_none() {
                                return Err(ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()));
                            }
                            if let Some(usage) = parser.usage {
                                send_event(&events, identity, AdapterEventKind::Usage(usage)).await?;
                            }
                            send_event(&events, identity, AdapterEventKind::ResponseFinished { response_id, finish_reason: Some("end_turn".to_string()), complete: true }).await?;
                            let output = parser.result.ok_or_else(|| ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()))?;
                            return Ok(NativeRunResult::Completed { session_id, output });
                        }
                    }
                }
                changed = cancellation.changed() => {
                    if changed.is_ok() && *cancellation.borrow() != identity.cancellation_generation {
                        terminate_child(&mut child, job).await;
                        let _ = stderr_task.await;
                        return Ok(NativeRunResult::Cancelled);
                    }
                    if changed.is_err() {
                        return Err(ProviderRuntimeError::Transport("provider_transport_closed".to_string()));
                    }
                }
            }
        }
    }
}

async fn read_bounded_jsonl_line<R>(
    reader: &mut R,
) -> Result<Option<(Vec<u8>, usize)>, ProviderRuntimeError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let limit =
        u64::try_from(super::request::MAX_JSONL_LINE_BYTES.saturating_add(2)).unwrap_or(u64::MAX);
    let read = reader
        .take(limit)
        .read_until(b'\n', &mut line)
        .await
        .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?;
    if read == 0 {
        return Ok(None);
    }
    let consumed_bytes = line.len();
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
    if line.len() > super::request::MAX_JSONL_LINE_BYTES {
        return Err(ProviderRuntimeError::Protocol(
            "provider_protocol_changed".to_string(),
        ));
    }
    Ok(Some((line, consumed_bytes)))
}
