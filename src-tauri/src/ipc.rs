//! Tauri IPC command and event payload schema.
//!
//! Panel-to-core commands are exposed through Tauri `invoke`, and core-to-panel messages
//! are emitted as typed Tauri events. The command bodies are placeholders until the engine
//! orchestration task wires RAG, Codex, LSP, and editor bridge calls into this surface.

use serde::{Deserialize, Serialize};
use tauri::Emitter;

const ENGINE_NOT_WIRED: &str = "engine IPC handler is not wired yet";

/// `instruct` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructRequest {
    /// User instruction from the panel.
    pub instruction: String,
    /// Target editor file/path.
    pub target: String,
    /// Whether to use RAG/project context.
    #[serde(rename = "useContext")]
    pub use_context: bool,
}

/// `apply` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyRequest {
    /// Apply mode: replace an existing settable file or create a new EPS file.
    pub mode: ApplyMode,
    /// Target editor file/path.
    pub target: String,
    /// Code to apply through the editor bridge.
    pub code: String,
}

/// `apply` mode wire values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApplyMode {
    /// Replace the target content.
    #[serde(rename = "set")]
    Set,
    /// Create a new EPS file.
    #[serde(rename = "neweps")]
    NewEps,
}

/// `status` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRequest {}

/// `list` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRequest {}

/// `status` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// True while EUD Editor is compiling.
    pub compiling: bool,
    /// Current project line from the editor status file.
    pub project: String,
}

/// `list` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse {
    /// Editor files exposed by the bridge LIST command.
    pub files: Vec<FileEntry>,
}

/// A file entry returned by `list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Editor-relative path.
    pub path: String,
    /// File type label from the bridge.
    pub ftype: String,
    /// True when the file can be changed through SET/NEWEPS.
    pub settable: bool,
}

/// `apply` command success output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResponse {
    /// Applied target editor file/path.
    pub target: String,
}

/// `progress` event payload.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    /// Current orchestration stage.
    pub stage: ProgressStage,
    /// Human-readable progress detail.
    pub detail: String,
    /// Optional percentage, omitted from the wire payload when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pct: Option<u8>,
}

/// `progress.stage` wire values.
#[derive(Debug, Clone, Serialize)]
pub enum ProgressStage {
    /// RAG lookup.
    #[serde(rename = "rag")]
    Rag,
    /// Background RAG/model warmup.
    #[serde(rename = "rag_warmup")]
    RagWarmup,
    /// Codex subprocess execution.
    #[serde(rename = "codex")]
    Codex,
    /// Advisory epscript-lsp diagnostics.
    #[serde(rename = "lsp")]
    Lsp,
    /// Waiting for an editor build to finish.
    #[serde(rename = "waiting_build")]
    WaitingBuild,
    /// Bootstrap asset setup.
    #[serde(rename = "bootstrap")]
    Bootstrap,
}

/// `code` event payload.
#[derive(Debug, Clone, Serialize)]
pub struct CodeEvent {
    /// Proposed code.
    pub code: String,
    /// Proposed code language.
    pub lang: CodeLang,
    /// Unified diff string.
    pub diff: String,
    /// Advisory diagnostics; empty when absent.
    pub diagnostics: Vec<serde_json::Value>,
}

/// `code.lang` wire values.
#[derive(Debug, Clone, Serialize)]
pub enum CodeLang {
    /// EPScript code.
    #[serde(rename = "eps")]
    Eps,
}

/// `agent_event` payload.
#[derive(Debug, Clone, Serialize)]
pub struct AgentEvent {
    /// Agent event kind; the panel must not render this raw string as user-facing text.
    pub kind: String,
    /// Arbitrary additional event fields from the agent stream.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// `applied` event payload.
#[derive(Debug, Clone, Serialize)]
pub struct AppliedEvent {
    /// Applied target editor file/path.
    pub target: String,
}

/// `error` event payload.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorEvent {
    /// User-facing error message.
    pub message: String,
}

/// Run RAG, Codex, diff, and diagnostics for an instruction.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn instruct<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    instruction: String,
    target: String,
    use_context: bool,
) -> Result<(), String> {
    let request = InstructRequest {
        instruction,
        target,
        use_context,
    };
    let _ = request;
    emit_error(
        &app,
        ErrorEvent {
            message: ENGINE_NOT_WIRED.to_string(),
        },
    )
    .map_err(|e| e.to_string())?;
    Err(ENGINE_NOT_WIRED.to_string())
}

/// Apply proposed code through the editor bridge.
///
/// The engine/bridge task replaces this placeholder body with the real apply flow.
#[tauri::command]
pub async fn apply<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    mode: ApplyMode,
    target: String,
    code: String,
) -> Result<ApplyResponse, String> {
    let request = ApplyRequest { mode, target, code };
    let _ = request;
    emit_error(
        &app,
        ErrorEvent {
            message: ENGINE_NOT_WIRED.to_string(),
        },
    )
    .map_err(|e| e.to_string())?;
    Err(ENGINE_NOT_WIRED.to_string())
}

/// Read editor compile/project status.
///
/// The bridge task replaces this placeholder body with status.txt parsing.
#[tauri::command]
pub async fn status() -> Result<StatusResponse, String> {
    Ok(StatusResponse {
        compiling: false,
        project: String::new(),
    })
}

/// List editor files available through the bridge.
///
/// The bridge task replaces this placeholder body with the LIST command.
#[tauri::command]
pub async fn list() -> Result<ListResponse, String> {
    Ok(ListResponse { files: Vec::new() })
}

/// Emit a `progress` event.
pub fn emit_progress<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: ProgressEvent,
) -> tauri::Result<()> {
    emitter.emit("progress", payload)
}

/// Emit a `code` event.
pub fn emit_code<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: CodeEvent,
) -> tauri::Result<()> {
    emitter.emit("code", payload)
}

/// Emit an `agent_event` event.
pub fn emit_agent_event<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: AgentEvent,
) -> tauri::Result<()> {
    emitter.emit("agent_event", payload)
}

/// Emit an `applied` event.
pub fn emit_applied<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: AppliedEvent,
) -> tauri::Result<()> {
    emitter.emit("applied", payload)
}

/// Emit an `error` event.
pub fn emit_error<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: ErrorEvent,
) -> tauri::Result<()> {
    emitter.emit("error", payload)
}

#[cfg(test)]
mod tests {
    use crate::ipc;
    use serde_json::json;

    fn assert_json<T: serde::Serialize>(value: &T, expected: serde_json::Value) {
        assert_eq!(serde_json::to_value(value).unwrap(), expected);
    }

    #[test]
    fn command_inputs_serialize_to_documented_wire_schema() {
        let instruct = ipc::InstructRequest {
            instruction: "Create a countdown trigger.".to_string(),
            target: "triggers/main.eps".to_string(),
            use_context: true,
        };
        assert_json(
            &instruct,
            json!({
                "instruction": "Create a countdown trigger.",
                "target": "triggers/main.eps",
                "useContext": true
            }),
        );

        let apply = ipc::ApplyRequest {
            mode: ipc::ApplyMode::NewEps,
            target: "triggers/generated.eps".to_string(),
            code: "function main() {}".to_string(),
        };
        assert_json(
            &apply,
            json!({
                "mode": "neweps",
                "target": "triggers/generated.eps",
                "code": "function main() {}"
            }),
        );

        assert_json(&ipc::StatusRequest {}, json!({}));
        assert_json(&ipc::ListRequest {}, json!({}));
    }

    #[test]
    fn apply_mode_uses_documented_string_values() {
        assert_json(&ipc::ApplyMode::Set, json!("set"));
        assert_json(&ipc::ApplyMode::NewEps, json!("neweps"));
    }

    #[test]
    fn command_outputs_serialize_to_documented_wire_schema() {
        let status = ipc::StatusResponse {
            compiling: false,
            project: "ExampleProject".to_string(),
        };
        assert_json(
            &status,
            json!({
                "compiling": false,
                "project": "ExampleProject"
            }),
        );

        let list = ipc::ListResponse {
            files: vec![
                ipc::FileEntry {
                    path: "triggers/main.eps".to_string(),
                    ftype: "eps".to_string(),
                    settable: true,
                },
                ipc::FileEntry {
                    path: "ui/layout.cui".to_string(),
                    ftype: "cui".to_string(),
                    settable: true,
                },
            ],
        };
        assert_json(
            &list,
            json!({
                "files": [
                    { "path": "triggers/main.eps", "ftype": "eps", "settable": true },
                    { "path": "ui/layout.cui", "ftype": "cui", "settable": true }
                ]
            }),
        );

        let applied = ipc::ApplyResponse {
            target: "triggers/generated.eps".to_string(),
        };
        assert_json(
            &applied,
            json!({
                "target": "triggers/generated.eps"
            }),
        );
    }

    #[test]
    fn progress_events_serialize_stages_and_skip_absent_pct() {
        let with_pct = ipc::ProgressEvent {
            stage: ipc::ProgressStage::RagWarmup,
            detail: "Loading embeddings".to_string(),
            pct: Some(25),
        };
        assert_json(
            &with_pct,
            json!({
                "stage": "rag_warmup",
                "detail": "Loading embeddings",
                "pct": 25
            }),
        );

        let without_pct = ipc::ProgressEvent {
            stage: ipc::ProgressStage::WaitingBuild,
            detail: "Editor build is still running".to_string(),
            pct: None,
        };
        assert_json(
            &without_pct,
            json!({
                "stage": "waiting_build",
                "detail": "Editor build is still running"
            }),
        );

        assert_json(&ipc::ProgressStage::Rag, json!("rag"));
        assert_json(&ipc::ProgressStage::Codex, json!("codex"));
        assert_json(&ipc::ProgressStage::Lsp, json!("lsp"));
        assert_json(&ipc::ProgressStage::Bootstrap, json!("bootstrap"));
    }

    #[test]
    fn code_and_agent_events_serialize_to_documented_wire_schema() {
        let code = ipc::CodeEvent {
            code: "function main() {}".to_string(),
            lang: ipc::CodeLang::Eps,
            diff: "--- old\n+++ new\n@@\n-function old() {}\n+function main() {}\n".to_string(),
            diagnostics: Vec::new(),
        };
        assert_json(
            &code,
            json!({
                "code": "function main() {}",
                "lang": "eps",
                "diff": "--- old\n+++ new\n@@\n-function old() {}\n+function main() {}\n",
                "diagnostics": []
            }),
        );

        let agent_event = ipc::AgentEvent {
            kind: "reasoning_delta".to_string(),
            extra: json!({
                "text": "Checking the target.",
                "sequence": 7
            }),
        };
        assert_json(
            &agent_event,
            json!({
                "kind": "reasoning_delta",
                "text": "Checking the target.",
                "sequence": 7
            }),
        );
    }

    #[test]
    fn applied_and_error_events_serialize_to_documented_wire_schema() {
        let applied = ipc::AppliedEvent {
            target: "triggers/main.eps".to_string(),
        };
        assert_json(
            &applied,
            json!({
                "target": "triggers/main.eps"
            }),
        );

        let error = ipc::ErrorEvent {
            message: "editor not connected".to_string(),
        };
        assert_json(
            &error,
            json!({
                "message": "editor not connected"
            }),
        );
    }
}
