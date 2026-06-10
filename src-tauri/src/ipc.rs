//! Tauri IPC command and event payload schema.
//!
//! Panel-to-core commands are exposed through Tauri `invoke`, and core-to-panel messages
//! are emitted as typed Tauri events. The command bodies are placeholders until the engine
//! orchestration task wires RAG, Codex, LSP, and editor bridge calls into this surface.

use std::path::Path;

use crate::bridge_io::{BridgeIo, SendOpts, HEARTBEAT_STALE_AFTER};
use crate::config::{self, DataDirs};
use serde::{Deserialize, Serialize};
use tauri::Emitter;

const EDITOR_NOT_CONNECTED: &str = "editor not connected";

/// Managed app data-dir state used by bridge-backed IPC commands.
///
/// Commands resolve `config.json` on every call so first-run/editor-path edits take effect
/// without restarting the Tauri app.
#[derive(Debug, Clone)]
pub struct BridgeManaged {
    dirs: DataDirs,
}

impl BridgeManaged {
    /// Create managed bridge state from resolved app data directories.
    pub fn new(dirs: DataDirs) -> Self {
        Self { dirs }
    }

    /// Resolved app data directories.
    pub fn dirs(&self) -> &DataDirs {
        &self.dirs
    }
}

/// Resolve a bridge client from `config.json`.
pub fn bridge_from_config(dirs: &DataDirs) -> Result<BridgeIo, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    let editor_path = config.editor_path.trim();
    if editor_path.is_empty() {
        return Err(EDITOR_NOT_CONNECTED.to_string());
    }
    Ok(BridgeIo::new(config::editor_ipc_dir(Path::new(
        editor_path,
    ))))
}

/// `chat` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// User message from the panel.
    pub text: String,
}

/// `plan_feedback` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanFeedbackRequest {
    /// User feedback for the current plan.
    pub text: String,
}

/// `changeset_decision.decision` wire values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Decision {
    /// Accept the targeted changeset items.
    #[serde(rename = "accept")]
    Accept,
    /// Reject and roll back the targeted changeset items.
    #[serde(rename = "reject")]
    Reject,
}

/// Marker that (de)serializes only as the exact literal `all`.
#[derive(Debug, Clone)]
pub struct AllLiteral;

impl Serialize for AllLiteral {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("all")
    }
}

impl<'de> Deserialize<'de> for AllLiteral {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "all" {
            Ok(AllLiteral)
        } else {
            Err(serde::de::Error::custom("expected the literal \"all\""))
        }
    }
}

/// `changeset_decision.ids` wire values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DecisionIds {
    /// The literal `all` for every pending changeset item.
    All(AllLiteral),
    /// Specific changeset item ids.
    List(Vec<String>),
}

/// `changeset_decision` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetDecisionRequest {
    /// Accept or reject the targeted changeset items.
    pub decision: Decision,
    /// Target all items or a specific list of item ids.
    pub ids: DecisionIds,
}

/// `status` command output and push event payload.
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

/// `agent_event.data` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEventData {
    /// Tool call argument text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Tool result or error text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Tool result status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// `agent_event` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Agent event kind; the panel must not render this raw string as user-facing text.
    pub kind: String,
    /// Short event detail or streamed delta text.
    pub detail: String,
    /// Optional tool call/result payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<AgentEventData>,
}

/// `answer` event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerEvent {
    /// Answer-only turn text.
    pub text: String,
}

/// `plan` event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEvent {
    /// Proposed plan markdown.
    pub markdown: String,
    /// Plan revision number.
    pub revision: u32,
}

/// One changeset item emitted for user accept/reject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetItem {
    /// Changeset category, such as `file`, `dat`, or a flat editor object type.
    pub category: String,
    /// Stable per-item id.
    pub id: String,
    /// Journal sequence number.
    pub seq: u32,
    /// Remaining core fields, preserved for panel rendering.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `changeset` event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetEvent {
    /// Request id shared by the emitted changeset.
    pub request_id: String,
    /// Journaled changeset items awaiting a decision.
    pub items: Vec<ChangesetItem>,
}

/// `rollback_result` event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackResultEvent {
    /// Item ids accepted or rolled back.
    pub ids: Vec<String>,
    /// True when the requested rollback/decision succeeded.
    pub ok: bool,
}

/// `progress` event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    /// Current orchestration stage.
    pub stage: ProgressStage,
    /// Human-readable progress detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `progress.stage` wire values.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// `error` event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEvent {
    /// User-facing error message.
    pub message: String,
}

/// Start a chat turn.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn chat(text: String) -> Result<(), String> {
    let _request = ChatRequest { text };
    Ok(())
}

/// Send feedback for the current plan.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn plan_feedback(text: String) -> Result<(), String> {
    let _request = PlanFeedbackRequest { text };
    Ok(())
}

/// Approve the current plan.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn plan_approve() -> Result<(), String> {
    Ok(())
}

/// Accept or reject pending changeset items.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn changeset_decision(decision: Decision, ids: DecisionIds) -> Result<(), String> {
    let _request = ChangesetDecisionRequest { decision, ids };
    Ok(())
}

/// Cancel the in-flight turn.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn cancel() -> Result<(), String> {
    Ok(())
}

/// Reset the conversation.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn reset() -> Result<(), String> {
    Ok(())
}

/// Read editor compile/project status.
#[tauri::command]
pub async fn status(state: tauri::State<'_, BridgeManaged>) -> Result<StatusResponse, String> {
    let bridge = bridge_from_config(state.dirs())?;
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        bridge.read_status_snapshot(HEARTBEAT_STALE_AFTER)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    Ok(StatusResponse {
        compiling: snapshot.compiling,
        project: snapshot.project,
    })
}

/// List editor files available through the bridge.
///
/// While the editor is compiling the bridge round-trip extends to the busy timeout; the
/// `on_busy` hook emits `progress {stage: waiting_build}` so the panel can surface the
/// build wait instead of appearing stuck (rules.md IPC timeout/progress contract).
#[tauri::command]
pub async fn list(
    app: tauri::AppHandle,
    state: tauri::State<'_, BridgeManaged>,
) -> Result<ListResponse, String> {
    let bridge = bridge_from_config(state.dirs())?;
    let files = tauri::async_runtime::spawn_blocking(move || {
        let opts = SendOpts::default();
        let on_busy = || {
            let _ = emit_progress(
                &app,
                ProgressEvent {
                    stage: ProgressStage::WaitingBuild,
                    detail: Some("editor build in progress".to_string()),
                },
            );
        };
        bridge.list_connected(&opts, Some(&on_busy), HEARTBEAT_STALE_AFTER)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?
    .into_iter()
    .map(|entry| FileEntry {
        path: entry.path,
        ftype: entry.ftype,
        settable: entry.settable,
    })
    .collect();

    Ok(ListResponse { files })
}

/// Emit an `agent_event` event.
pub fn emit_agent_event<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: AgentEvent,
) -> tauri::Result<()> {
    emitter.emit("agent_event", payload)
}

/// Emit an `answer` event.
pub fn emit_answer<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: AnswerEvent,
) -> tauri::Result<()> {
    emitter.emit("answer", payload)
}

/// Emit a `plan` event.
pub fn emit_plan<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: PlanEvent,
) -> tauri::Result<()> {
    emitter.emit("plan", payload)
}

/// Emit a `changeset` event.
pub fn emit_changeset<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: ChangesetEvent,
) -> tauri::Result<()> {
    emitter.emit("changeset", payload)
}

/// Emit a `rollback_result` event.
pub fn emit_rollback_result<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: RollbackResultEvent,
) -> tauri::Result<()> {
    emitter.emit("rollback_result", payload)
}

/// Emit a `progress` event.
pub fn emit_progress<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: ProgressEvent,
) -> tauri::Result<()> {
    emitter.emit("progress", payload)
}

/// Emit an `error` event.
pub fn emit_error<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: ErrorEvent,
) -> tauri::Result<()> {
    emitter.emit("error", payload)
}

/// Emit a `status` event.
pub fn emit_status<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: StatusResponse,
) -> tauri::Result<()> {
    emitter.emit("status", payload)
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, DataDirs};
    use crate::ipc;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    /// Unique temp base dir for a test, avoiding a `tempfile` dev-dependency
    /// (Cargo.toml is out of scope for this task).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-ipc-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_json<T: serde::Serialize>(value: &T, expected: serde_json::Value) {
        assert_eq!(serde_json::to_value(value).unwrap(), expected);
    }

    #[test]
    fn command_inputs_serialize_to_v2_wire_schema() {
        let chat: ipc::ChatRequest = serde_json::from_value(json!({ "text": "hi" })).unwrap();
        assert_json(&chat, json!({ "text": "hi" }));

        let feedback: ipc::PlanFeedbackRequest =
            serde_json::from_value(json!({ "text": "Please revise it." })).unwrap();
        assert_json(&feedback, json!({ "text": "Please revise it." }));

        assert_json(&ipc::Decision::Accept, json!("accept"));
        assert_json(&ipc::Decision::Reject, json!("reject"));

        let all_ids: ipc::DecisionIds = serde_json::from_value(json!("all")).unwrap();
        assert_json(&all_ids, json!("all"));

        let selected_ids: ipc::DecisionIds = serde_json::from_value(json!(["a", "b"])).unwrap();
        assert_json(&selected_ids, json!(["a", "b"]));

        let accept_all: ipc::ChangesetDecisionRequest =
            serde_json::from_value(json!({ "decision": "accept", "ids": "all" })).unwrap();
        assert_json(
            &accept_all,
            json!({
                "decision": "accept",
                "ids": "all"
            }),
        );

        let reject_selected: ipc::ChangesetDecisionRequest =
            serde_json::from_value(json!({ "decision": "reject", "ids": ["a", "b"] })).unwrap();
        assert_json(
            &reject_selected,
            json!({
                "decision": "reject",
                "ids": ["a", "b"]
            }),
        );
    }

    #[test]
    fn changeset_decision_ids_reject_non_all_bare_strings() {
        assert!(serde_json::from_value::<ipc::DecisionIds>(json!("a")).is_err());
        assert!(serde_json::from_value::<ipc::DecisionIds>(json!("")).is_err());
        assert!(serde_json::from_value::<ipc::DecisionIds>(json!("All")).is_err());
        assert!(
            serde_json::from_value::<ipc::ChangesetDecisionRequest>(json!({
                "decision": "accept",
                "ids": "nope"
            }))
            .is_err()
        );
    }

    #[test]
    fn status_and_list_outputs_match_v2_wire_schema() {
        let status: ipc::StatusResponse =
            serde_json::from_value(json!({ "compiling": false, "project": "ExampleProject" }))
                .unwrap();
        assert_json(
            &status,
            json!({
                "compiling": false,
                "project": "ExampleProject"
            }),
        );

        let list: ipc::ListResponse = serde_json::from_value(json!({
            "files": [
                { "path": "triggers/main.eps", "ftype": "eps", "settable": true },
                { "path": "ui/layout.cui", "ftype": "cui", "settable": true }
            ]
        }))
        .unwrap();
        assert_json(
            &list,
            json!({
                "files": [
                    { "path": "triggers/main.eps", "ftype": "eps", "settable": true },
                    { "path": "ui/layout.cui", "ftype": "cui", "settable": true }
                ]
            }),
        );
    }

    #[test]
    fn progress_events_serialize_stages_and_skip_absent_detail() {
        let with_detail = ipc::ProgressEvent {
            stage: ipc::ProgressStage::RagWarmup,
            detail: Some("Loading embeddings".to_string()),
        };
        assert_json(
            &with_detail,
            json!({
                "stage": "rag_warmup",
                "detail": "Loading embeddings"
            }),
        );

        let without_detail = ipc::ProgressEvent {
            stage: ipc::ProgressStage::WaitingBuild,
            detail: None,
        };
        assert_json(
            &without_detail,
            json!({
                "stage": "waiting_build"
            }),
        );

        assert_json(&ipc::ProgressStage::Rag, json!("rag"));
        assert_json(&ipc::ProgressStage::Codex, json!("codex"));
        assert_json(&ipc::ProgressStage::Lsp, json!("lsp"));
        assert_json(&ipc::ProgressStage::Bootstrap, json!("bootstrap"));
    }

    #[test]
    fn error_events_match_v2_wire_schema() {
        let error: ipc::ErrorEvent =
            serde_json::from_value(json!({ "message": "editor not connected" })).unwrap();
        assert_json(
            &error,
            json!({
                "message": "editor not connected"
            }),
        );
    }

    #[test]
    fn bridge_state_from_config_rejects_unset_editor_path() {
        let base = unique_temp_dir("unset-editor");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.save_config(&Config::default()).unwrap();

        let err = ipc::bridge_from_config(&dirs).unwrap_err();

        assert_eq!(err, "editor not connected");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn v2_payloads_match_panel_wire_schema() {
        let chat: ipc::ChatRequest = serde_json::from_value(json!({ "text": "hi" })).unwrap();
        assert_json(&chat, json!({ "text": "hi" }));

        assert_json(&ipc::Decision::Accept, json!("accept"));
        assert_json(&ipc::Decision::Reject, json!("reject"));

        let all_ids: ipc::DecisionIds = serde_json::from_value(json!("all")).unwrap();
        assert_json(&all_ids, json!("all"));

        let selected_ids: ipc::DecisionIds = serde_json::from_value(json!(["a", "b"])).unwrap();
        assert_json(&selected_ids, json!(["a", "b"]));

        let accept_all: ipc::ChangesetDecisionRequest =
            serde_json::from_value(json!({ "decision": "accept", "ids": "all" })).unwrap();
        assert_json(
            &accept_all,
            json!({
                "decision": "accept",
                "ids": "all"
            }),
        );

        let reject_selected: ipc::ChangesetDecisionRequest =
            serde_json::from_value(json!({ "decision": "reject", "ids": ["a", "b"] })).unwrap();
        assert_json(
            &reject_selected,
            json!({
                "decision": "reject",
                "ids": ["a", "b"]
            }),
        );

        let agent_event_without_data = ipc::AgentEvent {
            kind: "thinking".to_string(),
            detail: "Checking context".to_string(),
            data: None,
        };
        assert_json(
            &agent_event_without_data,
            json!({
                "kind": "thinking",
                "detail": "Checking context"
            }),
        );

        let agent_event_with_args = ipc::AgentEvent {
            kind: "tool_call".to_string(),
            detail: "search_docs".to_string(),
            data: Some(ipc::AgentEventData {
                args: Some("{\"query\":\"countdown\"}".to_string()),
                result: None,
                status: None,
            }),
        };
        assert_json(
            &agent_event_with_args,
            json!({
                "kind": "tool_call",
                "detail": "search_docs",
                "data": {
                    "args": "{\"query\":\"countdown\"}"
                }
            }),
        );

        let agent_event_with_result = ipc::AgentEvent {
            kind: "tool_result".to_string(),
            detail: "search_docs".to_string(),
            data: Some(ipc::AgentEventData {
                args: None,
                result: Some("2 hits".to_string()),
                status: Some("completed".to_string()),
            }),
        };
        assert_json(
            &agent_event_with_result,
            json!({
                "kind": "tool_result",
                "detail": "search_docs",
                "data": {
                    "result": "2 hits",
                    "status": "completed"
                }
            }),
        );

        let answer = ipc::AnswerEvent {
            text: "No edits are needed.".to_string(),
        };
        assert_json(
            &answer,
            json!({
                "text": "No edits are needed."
            }),
        );

        let plan = ipc::PlanEvent {
            markdown: "- Update the trigger\n- Verify compile".to_string(),
            revision: 2,
        };
        assert_json(
            &plan,
            json!({
                "markdown": "- Update the trigger\n- Verify compile",
                "revision": 2
            }),
        );

        let changeset = ipc::ChangesetEvent {
            request_id: "req-1".to_string(),
            items: vec![ipc::ChangesetItem {
                category: "file".to_string(),
                id: "a".to_string(),
                seq: 1,
                extra: [("path".to_string(), json!("triggers/main.eps"))]
                    .into_iter()
                    .collect(),
            }],
        };
        assert_json(
            &changeset,
            json!({
                "request_id": "req-1",
                "items": [
                    {
                        "category": "file",
                        "id": "a",
                        "seq": 1,
                        "path": "triggers/main.eps"
                    }
                ]
            }),
        );

        let rollback = ipc::RollbackResultEvent {
            ids: vec!["a".to_string(), "b".to_string()],
            ok: true,
        };
        assert_json(
            &rollback,
            json!({
                "ids": ["a", "b"],
                "ok": true
            }),
        );

        let progress_with_detail = ipc::ProgressEvent {
            stage: ipc::ProgressStage::Codex,
            detail: Some("Generating patch".to_string()),
        };
        assert_json(
            &progress_with_detail,
            json!({
                "stage": "codex",
                "detail": "Generating patch"
            }),
        );

        let progress_without_detail = ipc::ProgressEvent {
            stage: ipc::ProgressStage::Bootstrap,
            detail: None,
        };
        assert_json(
            &progress_without_detail,
            json!({
                "stage": "bootstrap"
            }),
        );
    }
}
