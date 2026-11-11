use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

use crate::provider_runtime::{
    AdapterEvent, AdapterEventKind, NormalizedBlock, ProviderRuntimeError, RunIdentity,
};

use super::events::{send_event, ParsedEvent};
use super::foreground::NativeRunResult;
use super::parser::{boundary_violation, tool_list, ClaudeStreamParser};
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
    /// The session's steer inbox. With one, stdin stays open for the turn so
    /// steers can follow the prompt, and the CLI runs with
    /// `--replay-user-messages`, whose echo of a steer marks where the model
    /// received it.
    pub(super) steer: Option<crate::steer::SteerInbox>,
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
            steer,
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
        // Without steering the prompt is the whole input. With it, stdin stays
        // open until the result: the CLI reads further user lines as steers,
        // and closing stdin is what lets it exit after the turn. A writer task
        // owns stdin then, so a large steer never blocks reading stdout or
        // reacting to a cancel; dropping its sender closes stdin.
        let mut stdin = if steer.is_some() {
            stdin.flush().await.map_err(|_| {
                ProviderRuntimeError::Transport("provider_transport_closed".to_string())
            })?;
            Some(spawn_stdin_writer(stdin))
        } else {
            stdin.shutdown().await.map_err(|_| {
                ProviderRuntimeError::Transport("provider_transport_closed".to_string())
            })?;
            None
        };
        let mut steers = SteerDelivery::new(steer);
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
        let mut ended_after_result = false;
        loop {
            tokio::select! {
                read = read_bounded_jsonl_line(&mut stdout) => {
                    match read? {
                        Some((line, consumed_bytes)) => {
                            // The process was ended at its result because a steer
                            // was never echoed: whatever it still printed is a
                            // second turn it began, not part of this one.
                            if ended_after_result {
                                continue;
                            }
                            total_bytes = total_bytes.saturating_add(consumed_bytes);
                            if total_bytes > max_output_bytes {
                                terminate_child(&mut child, job).await;
                                let _ = stderr_task.await;
                                return Err(ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()));
                            }
                            // A line that is not JSON (a warning the CLI or a hook
                            // printed) is no part of the stream.
                            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                                continue;
                            };
                            if !parser.has_result() && super::parser::is_user_replay(&value) {
                                if let Some(steer) = steers.echoed(&value) {
                                    if events.is_some() {
                                        send_event(&events, identity, AdapterEventKind::SteerApplied(vec![steer])).await?;
                                    } else if let Some(inbox) = steers.inbox() {
                                        inbox.applied(std::slice::from_ref(&steer));
                                    }
                                }
                            }
                            if value.pointer("/event/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                                let name = value.pointer("/event/content_block/name").and_then(Value::as_str);
                                if !name.is_some_and(|name| parser.admits_tool(name, require_mcp, None)) {
                                    return Err(boundary_violation(format_args!(
                                        "model called unopened tool {}",
                                        tool_list(&[name.unwrap_or("<unnamed>")])
                                    )));
                                }
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
                            let had_result = parser.has_result();
                            let parsed = parser.apply(&value)?;
                            if parser.has_result() && !had_result {
                                // The turn is over: close stdin so the CLI exits. A
                                // steer it has not echoed yet would start a second
                                // turn in this process, so the process ends here and
                                // the steer waits for the engine's next turn.
                                stdin = None;
                                if steers.end_turn() {
                                    let _ = child.start_kill();
                                    ended_after_result = true;
                                }
                            }
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
                                if let ParsedEvent::ToolObservation { name, status, .. } = &event {
                                    if !parser.admits_tool(name, require_mcp, status.as_deref()) {
                                        return Err(boundary_violation(format_args!(
                                            "CLI reported unopened tool {}",
                                            tool_list(&[name.as_str()])
                                        )));
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
                            steers.end_turn();
                            if !(status.success() || ended_after_result) || parser.is_error {
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
                () = crate::steer::wait_optional(steers.inbox()), if stdin.is_some() => {
                    if let Some(writer) = stdin.as_ref() {
                        if !steers.write_queued(writer) {
                            stdin = None;
                        }
                    }
                }
                changed = cancellation.changed() => {
                    if changed.is_ok() && *cancellation.borrow() != identity.cancellation_generation {
                        steers.end_turn();
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

/// The steers one Claude Code process received: written to its stdin, then
/// applied when the CLI echoes them back.
struct SteerDelivery {
    inbox: Option<crate::steer::SteerInbox>,
    /// Written to stdin, not echoed yet.
    written: Vec<crate::steer::SteerMessage>,
}

impl SteerDelivery {
    fn new(inbox: Option<crate::steer::SteerInbox>) -> Self {
        Self {
            inbox,
            written: Vec::new(),
        }
    }

    fn inbox(&self) -> Option<&crate::steer::SteerInbox> {
        self.inbox.as_ref()
    }

    /// Hand every queued steer to the stdin writer. Returns whether stdin
    /// still takes steers; when the writer is gone the rest go back to the
    /// inbox. Never waits on the pipe.
    fn write_queued(&mut self, stdin: &StdinWriter) -> bool {
        let Some(inbox) = self.inbox.clone() else {
            return false;
        };
        let mut queued = inbox.drain().into_iter();
        while let Some(steer) = queued.next() {
            let sent = match steer_line(&steer) {
                Ok(line) => stdin.send(line).is_ok(),
                Err(_) => false,
            };
            if !sent {
                inbox.requeue(std::iter::once(steer).chain(queued).collect());
                return false;
            }
            self.written.push(steer);
        }
        true
    }

    /// The CLI echoed a user line: the steer it names reached the model here.
    fn echoed(&mut self, value: &Value) -> Option<crate::steer::SteerMessage> {
        let uuid = value.get("uuid").and_then(Value::as_str)?;
        let index = self
            .written
            .iter()
            .position(|steer| steer.steer_id == uuid)?;
        Some(self.written.remove(index))
    }

    /// The turn ended: steers written but never echoed were not received,
    /// so they go back to the inbox. Returns whether there were any.
    fn end_turn(&mut self) -> bool {
        let unconsumed = std::mem::take(&mut self.written);
        let any = !unconsumed.is_empty();
        if let Some(inbox) = &self.inbox {
            inbox.requeue(unconsumed);
        }
        any
    }
}

type StdinWriter = tokio::sync::mpsc::UnboundedSender<Vec<u8>>;

/// Own the CLI's stdin on a task that writes each line it is handed. Dropping
/// the returned sender closes stdin once the queued lines are written; a
/// failed write ends the task, so later sends fail and their steers stay
/// unconsumed.
fn spawn_stdin_writer(mut stdin: tokio::process::ChildStdin) -> StdinWriter {
    let (sender, mut lines) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(line) = lines.recv().await {
            if stdin.write_all(&line).await.is_err() || stdin.flush().await.is_err() {
                return;
            }
        }
        let _ = stdin.shutdown().await;
    });
    sender
}

/// One steer as the CLI's stream-json input line: a user message the CLI takes
/// at its next step (`priority: next`) and echoes back under the same `uuid`.
fn steer_line(steer: &crate::steer::SteerMessage) -> Result<Vec<u8>, ProviderRuntimeError> {
    let mut turn = crate::provider_runtime::AgentTurnInput::text(steer.model_text.clone());
    turn.image_paths.clone_from(&steer.image_paths);
    let mut message = super::request::claude_user_message(&turn)
        .map_err(|_| ProviderRuntimeError::Protocol("provider image is unsupported".to_string()))?;
    message["uuid"] = Value::String(steer.steer_id.clone());
    message["priority"] = Value::String("next".to_string());
    message["session_id"] = Value::String(String::new());
    let mut line = serde_json::to_vec(&message)
        .map_err(|_| ProviderRuntimeError::Protocol("provider_protocol_changed".to_string()))?;
    line.push(b'\n');
    Ok(line)
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
