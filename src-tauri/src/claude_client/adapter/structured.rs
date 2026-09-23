use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::provider::ProviderId;
use crate::provider_runtime::{
    AdapterEvent, AdapterEventKind, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest,
    NormalizedBlock, ProviderRuntimeError,
};
use crate::provider_tool_loop::validate_structured_output;

use super::request::{
    hide_console, model_args, terminate_child, validate_workspace_boundary, MAX_STDERR_BYTES,
    MAX_STDOUT_BYTES,
};
use super::state::ProductionClaudeCodeAdapter;

impl ProductionClaudeCodeAdapter {
    pub(super) async fn run_structured(
        &mut self,
        mut request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> Result<AdapterStepOutcome, ProviderRuntimeError> {
        if *request.cancellation.borrow_and_update() != request.identity.cancellation_generation {
            return Ok(AdapterStepOutcome::Cancelled);
        }
        let AdapterRequestKind::Structured {
            prompt,
            workspace_root,
            output_schema,
            ..
        } = &request.kind
        else {
            return Err(ProviderRuntimeError::Protocol(
                "provider_protocol_changed".to_string(),
            ));
        };
        if request.native_mcp_endpoint.is_some()
            || !request.prior_tool_results.is_empty()
            || !request.tool_descriptors.is_empty()
        {
            return Err(ProviderRuntimeError::Protocol(
                "structured Claude job was granted tool authority".to_string(),
            ));
        }
        validate_workspace_boundary(workspace_root).map_err(ProviderRuntimeError::Protocol)?;
        if request.binding.model != self.model || request.binding.provider != ProviderId::ClaudeCode
        {
            return Err(ProviderRuntimeError::Protocol(
                "provider_model_unavailable".to_string(),
            ));
        }
        let model_args = model_args(
            &self.model,
            request
                .binding
                .reasoning
                .as_ref()
                .map(|selection| selection.level.as_str()),
        )
        .map_err(ProviderRuntimeError::Protocol)?;
        // Windows caps a whole command line at 32,767 characters while a harness prompt
        // carries up to 192 KiB of accepted context, so an argument prompt makes the spawn
        // itself fail. The prompt travels on stdin like the foreground stream's message
        // does; `-p` with no argument reads it from there.
        let prompt = prompt.clone().into_bytes();
        let mut command = self.command();
        command
            .arg("-p")
            .args(model_args)
            .args(["--output-format", "json"])
            .arg("--json-schema")
            .arg(output_schema.to_string())
            .arg("--tools")
            .arg("")
            .arg("--strict-mcp-config")
            .arg("--no-session-persistence")
            .args(["--permission-mode", "dontAsk"])
            .arg("--disable-slash-commands")
            .arg("--no-chrome")
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut command);
        let mut child = command.spawn().map_err(|_| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let mut job = Some(
            crate::provider_process::WindowsJob::assign(&child).map_err(|_| {
                ProviderRuntimeError::Transport(
                    "provider process isolation unavailable".to_string(),
                )
            })?,
        );
        let mut stdin = child.stdin.take().ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let max_output_bytes = MAX_STDOUT_BYTES;
        let mut stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout
                .take(u64::try_from(max_output_bytes.saturating_add(1)).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
                .await
                .map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.take(MAX_STDERR_BYTES).read_to_end(&mut bytes).await;
            bytes
        });
        // Both readers are already draining, so a prompt larger than the stdin pipe buffer
        // cannot deadlock against a CLI that writes before it has consumed all of its input.
        let written = async {
            stdin.write_all(&prompt).await?;
            stdin.shutdown().await
        }
        .await;
        drop(stdin);
        if written.is_err() {
            if let Some(job) = job.take() {
                terminate_child(&mut child, job).await;
            }
            stdout_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(ProviderRuntimeError::Transport(
                "provider_transport_closed".to_string(),
            ));
        }
        let generation = request.identity.cancellation_generation;
        let mut status = None;
        let mut stdout_bytes = None;
        while status.is_none() || stdout_bytes.is_none() {
            tokio::select! {
                output = &mut stdout_task, if stdout_bytes.is_none() => {
                    let bytes = output
                        .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?
                        .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?;
                    if bytes.len() > max_output_bytes {
                        if let Some(job) = job.take() {
                            terminate_child(&mut child, job).await;
                        }
                        let _ = stderr_task.await;
                        return Err(ProviderRuntimeError::StructuredOutputInvalid);
                    }
                    stdout_bytes = Some(bytes);
                }
                result = child.wait(), if status.is_none() => {
                    status = Some(result.map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?);
                }
                changed = request.cancellation.changed() => {
                    if changed.is_ok() && *request.cancellation.borrow() != generation {
                        if let Some(job) = job.take() {
                            terminate_child(&mut child, job).await;
                        }
                        if stdout_bytes.is_none() {
                            stdout_task.abort();
                            let _ = stdout_task.await;
                        }
                        let _ = stderr_task.await;
                        return Ok(AdapterStepOutcome::Cancelled);
                    }
                    if changed.is_err() {
                        return Err(ProviderRuntimeError::Transport("provider_transport_closed".to_string()));
                    }
                }
            }
        }
        drop(job);
        let _ = stderr_task.await;
        let status = status.ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        let stdout = stdout_bytes.ok_or_else(|| {
            ProviderRuntimeError::Transport("provider_transport_closed".to_string())
        })?;
        if !status.success() {
            return Err(ProviderRuntimeError::StructuredOutputInvalid);
        }
        let value: Value = serde_json::from_slice(&stdout)
            .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?;
        if value.get("type").and_then(Value::as_str) != Some("result")
            || value.get("subtype").and_then(Value::as_str) != Some("success")
            || value.get("is_error").and_then(Value::as_bool) != Some(false)
        {
            return Err(ProviderRuntimeError::StructuredOutputInvalid);
        }
        let structured = value
            .get("structured_output")
            .cloned()
            .ok_or(ProviderRuntimeError::StructuredOutputInvalid)?;
        validate_structured_output(output_schema, &structured)
            .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?;
        let response_id = format!("claude-{}", request.identity.run_id.get());
        events
            .send(AdapterEvent {
                identity: request.identity.clone(),
                kind: AdapterEventKind::ResponseStarted {
                    response_id: response_id.clone(),
                },
            })
            .await
            .map_err(|_| ProviderRuntimeError::StaleEvent)?;
        events
            .send(AdapterEvent {
                identity: request.identity.clone(),
                kind: AdapterEventKind::Block(NormalizedBlock::Text {
                    response_id: response_id.clone(),
                    text: serde_json::to_string(&structured)
                        .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?,
                }),
            })
            .await
            .map_err(|_| ProviderRuntimeError::StaleEvent)?;
        events
            .send(AdapterEvent {
                identity: request.identity,
                kind: AdapterEventKind::ResponseFinished {
                    response_id,
                    finish_reason: Some("structured".to_string()),
                    complete: true,
                },
            })
            .await
            .map_err(|_| ProviderRuntimeError::StaleEvent)?;
        Ok(AdapterStepOutcome::Completed {
            output: crate::provider_runtime::AdapterOutput::Structured(structured),
            continuation: None,
            native_conversation: None,
        })
    }
}
