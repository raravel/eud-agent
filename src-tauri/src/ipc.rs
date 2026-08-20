//! Tauri IPC command and event payload schema.
//!
//! Panel-to-core commands are exposed through Tauri `invoke`, and core-to-panel messages
//! are emitted as typed Tauri events. The command bodies are placeholders until the engine
//! orchestration task wires RAG, Codex, LSP, and editor bridge calls into this surface.

use std::path::Path;

use crate::bridge_io::{BridgeIo, SendOpts, HEARTBEAT_STALE_AFTER};
use crate::config::{self, DataDirs};
use crate::memory::ProjectMemory;
use serde::{Deserialize, Serialize};
use tauri::Emitter;

/// Distinct from bridge_io's "editor not connected" (stale heartbeat): an empty
/// `config.json` editor path means first-run setup has not happened — the panel
/// routes this to the setup screen, not the reconnect notice.
const EDITOR_PATH_NOT_CONFIGURED: &str = "editor path not configured";

/// The single shipped EUD Editor 3 binary at the install root. `editor_path` is
/// validated to contain `Data\Lua\TriggerEditor` (config::validate_editor_path), and the
/// install ships exactly one top-level exe by this name, so `launch_editor` resolves it by
/// a fixed name rather than scanning.
const EDITOR_EXE_NAME: &str = "EUD Editor 3.exe";

/// Stable error code: the editor path is configured but the expected exe is absent (a
/// moved/renamed install). The panel maps this to Korean text, never rendering the code.
const EDITOR_EXE_NOT_FOUND: &str = "editor executable not found";

/// Resolve the launchable editor exe under `editor_path`, or a stable error code.
///
/// Pure (string + filesystem-probe only) so the launch command's path/validation logic is
/// unit-testable without spawning the real editor: empty path → [`EDITOR_PATH_NOT_CONFIGURED`],
/// missing exe → [`EDITOR_EXE_NOT_FOUND`], otherwise the absolute exe path.
fn resolve_editor_exe(editor_path: &str) -> Result<std::path::PathBuf, String> {
    let editor_path = editor_path.trim();
    if editor_path.is_empty() {
        return Err(EDITOR_PATH_NOT_CONFIGURED.to_string());
    }
    let exe = Path::new(editor_path).join(EDITOR_EXE_NAME);
    if !exe.is_file() {
        return Err(EDITOR_EXE_NOT_FOUND.to_string());
    }
    Ok(exe)
}

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
        return Err(EDITOR_PATH_NOT_CONFIGURED.to_string());
    }
    Ok(BridgeIo::new(config::editor_ipc_dir(Path::new(
        editor_path,
    ))))
}

/// `chat` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// User message from the panel. May be empty when at least one attachment exists.
    pub text: String,
    /// Opaque app-owned attachment ids staged before this turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

/// `plan_feedback` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanFeedbackRequest {
    /// User feedback for the current plan.
    pub text: String,
    /// Opaque app-owned attachment ids staged before this turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
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

/// `memory_get` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGetResponse {
    /// Current project name from editor STATUS.
    pub project: String,
    /// Project memory markdown files.
    pub files: MemoryFiles,
    /// Recent episodes, newest first.
    pub episodes: Vec<serde_json::Value>,
}

/// Project memory markdown file payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFiles {
    /// Resource ledger.
    pub resources: String,
    /// Project/file structure notes.
    pub structure: String,
    /// Naming and trigger conventions.
    pub conventions: String,
    /// User corrections and durable lessons.
    pub lessons: String,
}

/// `memory_save` command input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySaveRequest {
    /// Memory file name (`resources`, `structure`, `conventions`, or `lessons`).
    pub file: String,
    /// Full replacement markdown content.
    pub content: String,
}

/// `memory_save` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySaveResponse {
    /// Saved memory file name.
    pub file: String,
}

/// `workspace_list` command output for the current editor project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListResponse {
    pub project: String,
    pub workspace_id: String,
    pub files: Vec<crate::workspace::WorkspaceFileEntry>,
}

/// `workspace_read` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceReadResponse {
    pub workspace_id: String,
    pub path: String,
    pub source: bool,
    pub content: String,
}

/// `wiki` event payload AND the `wiki_get` command output: the dat-edit ledger.
///
/// `entries` is a key -> entry map mirroring the on-disk `ledger.json`
/// (`"{table}:{dat}:{objId}:{property}"` keys). The panel keeps its WikiView in sync
/// from this after every accept that changes the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiResponse {
    /// Ledger schema version (currently 1).
    pub version: u32,
    /// Key -> ledger entry.
    pub entries: std::collections::BTreeMap<String, crate::wiki::LedgerEntry>,
}

impl From<&crate::wiki::Ledger> for WikiResponse {
    fn from(ledger: &crate::wiki::Ledger) -> Self {
        Self {
            version: ledger.version,
            entries: ledger.entries.clone(),
        }
    }
}

/// `wiki_save` command input: user-corrected ledger entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiSaveRequest {
    /// Key -> entry, as edited in the panel. Persisted with `editedByUser=true`.
    pub entries: std::collections::BTreeMap<String, crate::wiki::LedgerEntry>,
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

/// `session_loaded` event payload (session restore): a signal only. Carries the
/// loaded session id so the panel can correlate, never a rendered-raw kind string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLoadedEvent {
    /// The id of the session that just finished loading.
    pub id: String,
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
    /// Per-project filesystem + exact-root Windows sandbox setup.
    #[serde(rename = "workspace")]
    Workspace,
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
pub async fn chat(text: String, attachments: Vec<String>) -> Result<(), String> {
    let _request = ChatRequest { text, attachments };
    Ok(())
}

/// Send feedback for the current plan.
///
/// The engine task replaces this placeholder body with the real orchestration.
#[tauri::command]
pub async fn plan_feedback(text: String, attachments: Vec<String>) -> Result<(), String> {
    let _request = PlanFeedbackRequest { text, attachments };
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

/// Launch the configured EUD Editor 3 install.
///
/// Resolves `<editor_path>\EUD Editor 3.exe` and spawns it detached, with the install root
/// as the working directory (mirroring a double-click so the editor finds its `Data\`).
/// We never wait on or keep the child handle: the app's lifecycle is independent of the
/// editor (architecture.md), and the panel's existing heartbeat poll flips
/// `editorConnected` once the bridge comes up — that is the "connected" half of done.
/// Errors return a stable code (`editor path not configured` / `editor executable not
/// found`) the panel maps to Korean. The editor is third-party and only ever launched,
/// never modified.
#[tauri::command]
pub async fn launch_editor(state: tauri::State<'_, BridgeManaged>) -> Result<(), String> {
    let config = state
        .dirs()
        .load_config()
        .map_err(|error| error.to_string())?;
    let exe = resolve_editor_exe(&config.editor_path)?;
    // The install root is the exe's parent; use it as cwd.
    let cwd = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    // Spawn off the IPC thread; we drop the child so it runs independently of the app.
    tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new(&exe)
            .current_dir(&cwd)
            .spawn()
            .map(|_child| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
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

/// Build the `memory_get` payload for an already-resolved project name.
pub fn memory_get_payload(dirs: &DataDirs, project: &str) -> Result<MemoryGetResponse, String> {
    let memory = ProjectMemory::new(dirs.memory_dir(), project);
    if !memory.enabled() {
        return Err("no project is open; memory is disabled".to_string());
    }

    let mut episodes = memory.read_episodes(50);
    episodes.reverse();

    Ok(MemoryGetResponse {
        project: project.to_string(),
        files: MemoryFiles {
            resources: memory.read("resources"),
            structure: memory.read("structure"),
            conventions: memory.read("conventions"),
            lessons: memory.read("lessons"),
        },
        episodes,
    })
}

/// Save one project memory file for an already-resolved project name.
pub fn memory_save_payload(
    dirs: &DataDirs,
    project: &str,
    file: &str,
    content: &str,
) -> Result<MemorySaveResponse, String> {
    let memory = ProjectMemory::new(dirs.memory_dir(), project);
    let result = memory.write(file, content);
    if !result.ok {
        return Err(result.reason);
    }

    Ok(MemorySaveResponse {
        file: file.to_string(),
    })
}

/// Read project memory for the current editor project.
#[tauri::command]
pub async fn memory_get(
    state: tauri::State<'_, BridgeManaged>,
) -> Result<MemoryGetResponse, String> {
    let project = current_project_from_status(state.dirs()).await?;
    memory_get_payload(state.dirs(), &project)
}

/// Save one memory file for the current editor project.
#[tauri::command]
pub(crate) async fn memory_save(
    state: tauri::State<'_, BridgeManaged>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    file: String,
    content: String,
) -> Result<MemorySaveResponse, String> {
    let request = MemorySaveRequest { file, content };
    let project = current_project_from_status(state.dirs()).await?;
    let dirs = state.dirs().clone();
    let project_for_write = project.clone();
    engines
        .direct_project_write(&project, move || {
            memory_save_payload(&dirs, &project_for_write, &request.file, &request.content)
        })
        .await
}

/// Build the `wiki_get` payload (the current ledger) for a resolved project name.
pub fn wiki_get_payload(dirs: &DataDirs, project: &str) -> Result<WikiResponse, String> {
    let store = crate::wiki::WikiStore::load(dirs.wiki_dir(project));
    if !store.enabled() {
        return Err("no project is open; the wiki is disabled".to_string());
    }
    Ok(WikiResponse::from(store.ledger()))
}

/// Persist user-corrected ledger entries (sets `editedByUser=true`) for a project.
pub fn wiki_save_payload(
    dirs: &DataDirs,
    project: &str,
    entries: std::collections::BTreeMap<String, crate::wiki::LedgerEntry>,
) -> Result<WikiResponse, String> {
    let mut store = crate::wiki::WikiStore::load(dirs.wiki_dir(project));
    if !store.enabled() {
        return Err("no project is open; the wiki is disabled".to_string());
    }
    for mut entry in entries.into_values() {
        // Mirror the accept-hook invariant (wiki::accepted_ledger_entries): never
        // persist a null-valued entry. The panel's isLedgerEntry guard rejects
        // null, which would silently drop the whole wiki payload on the next sync.
        if entry.value.is_null() {
            continue;
        }
        entry.edited_by_user = true;
        store.upsert(entry);
    }
    store.save().map_err(|error| error.to_string())?;
    Ok(WikiResponse::from(store.ledger()))
}

/// Read the dat-edit wiki ledger for the current editor project.
#[tauri::command]
pub async fn wiki_get(state: tauri::State<'_, BridgeManaged>) -> Result<WikiResponse, String> {
    let project = current_project_from_status(state.dirs()).await?;
    wiki_get_payload(state.dirs(), &project)
}

/// Save user-corrected wiki entries for the current editor project.
#[tauri::command]
pub(crate) async fn wiki_save(
    state: tauri::State<'_, BridgeManaged>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    entries: std::collections::BTreeMap<String, crate::wiki::LedgerEntry>,
) -> Result<WikiResponse, String> {
    let request = WikiSaveRequest { entries };
    let project = current_project_from_status(state.dirs()).await?;
    let dirs = state.dirs().clone();
    let project_for_write = project.clone();
    engines
        .direct_project_write(&project, move || {
            wiki_save_payload(&dirs, &project_for_write, request.entries)
        })
        .await
}

/// Refresh and list the current project's real Codex workspace.
#[tauri::command]
pub async fn workspace_list(
    state: tauri::State<'_, BridgeManaged>,
) -> Result<WorkspaceListResponse, String> {
    let manager = crate::workspace::WorkspaceManager::new(state.dirs().clone());
    tauri::async_runtime::spawn_blocking(move || {
        let workspace = manager.prepare_current()?;
        let files = manager
            .list_files(&workspace)
            .map_err(|error| error.to_string())?;
        Ok(WorkspaceListResponse {
            project: workspace.project,
            workspace_id: workspace.id,
            files,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Read one UTF-8 workspace file by opaque workspace id + confined relative path.
#[tauri::command]
pub async fn workspace_read(
    state: tauri::State<'_, BridgeManaged>,
    workspace_id: String,
    path: String,
) -> Result<WorkspaceReadResponse, String> {
    let manager = crate::workspace::WorkspaceManager::new(state.dirs().clone());
    tauri::async_runtime::spawn_blocking(move || {
        let content = manager
            .read_file(&workspace_id, &path)
            .map_err(|error| error.to_string())?;
        let source = path == crate::workspace::SOURCE_DIR
            || path.starts_with(&format!("{}/", crate::workspace::SOURCE_DIR));
        Ok(WorkspaceReadResponse {
            workspace_id,
            path,
            source,
            content,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn current_project_from_status(dirs: &DataDirs) -> Result<String, String> {
    let bridge = bridge_from_config(dirs)?;
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        bridge.read_status_snapshot(HEARTBEAT_STALE_AFTER)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    Ok(snapshot.project)
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

/// Emit a `wiki` event carrying the dat-edit ledger after an accept changed it.
pub fn emit_wiki<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: WikiResponse,
) -> tauri::Result<()> {
    emitter.emit("wiki", payload)
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

/// Emit a `status` event.
pub fn emit_status<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: StatusResponse,
) -> tauri::Result<()> {
    emitter.emit("status", payload)
}

/// Emit a `session_loaded` event (session restore signal).
pub fn emit_session_loaded<R: tauri::Runtime>(
    emitter: &impl Emitter<R>,
    payload: SessionLoadedEvent,
) -> tauri::Result<()> {
    emitter.emit("session_loaded", payload)
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, DataDirs};
    use crate::ipc;
    use crate::memory::{ProjectMemory, CONTENT_CAP_BYTES};
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
        let chat: ipc::ChatRequest = serde_json::from_value(json!({
            "text": "hi",
            "attachments": ["d61bb417-728e-4db9-8d81-2b366201aa89"]
        }))
        .unwrap();
        assert_json(
            &chat,
            json!({
                "text": "hi",
                "attachments": ["d61bb417-728e-4db9-8d81-2b366201aa89"]
            }),
        );

        let feedback: ipc::PlanFeedbackRequest = serde_json::from_value(json!({
            "text": "Please revise it.",
            "attachments": ["bf93c8d0-76de-45bd-b118-2d004d71126e"]
        }))
        .unwrap();
        assert_json(
            &feedback,
            json!({
                "text": "Please revise it.",
                "attachments": ["bf93c8d0-76de-45bd-b118-2d004d71126e"]
            }),
        );

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
    fn memory_payloads_match_panel_wire_schema() {
        let memory: ipc::MemoryGetResponse = serde_json::from_value(json!({
            "project": "ExampleProject",
            "files": {
                "resources": "Switch 1 = boss phase",
                "structure": "main.eps: entry point",
                "conventions": "Use dc_ prefix for counters",
                "lessons": "Build after eps edits"
            },
            "episodes": [
                {
                    "ts": "2026-06-10T12:00:00Z",
                    "request_id": "req-1",
                    "instruction": "Add a boss phase",
                    "kind": "changeset",
                    "tools": ["file_write"],
                    "files": ["main.eps"],
                    "decision": "accepted"
                }
            ]
        }))
        .unwrap();
        assert_json(
            &memory,
            json!({
                "project": "ExampleProject",
                "files": {
                    "resources": "Switch 1 = boss phase",
                    "structure": "main.eps: entry point",
                    "conventions": "Use dc_ prefix for counters",
                    "lessons": "Build after eps edits"
                },
                "episodes": [
                    {
                        "ts": "2026-06-10T12:00:00Z",
                        "request_id": "req-1",
                        "instruction": "Add a boss phase",
                        "kind": "changeset",
                        "tools": ["file_write"],
                        "files": ["main.eps"],
                        "decision": "accepted"
                    }
                ]
            }),
        );

        let save: ipc::MemorySaveRequest =
            serde_json::from_value(json!({ "file": "resources", "content": "Switch 2" })).unwrap();
        assert_json(
            &save,
            json!({
                "file": "resources",
                "content": "Switch 2"
            }),
        );

        let saved: ipc::MemorySaveResponse =
            serde_json::from_value(json!({ "file": "resources" })).unwrap();
        assert_json(&saved, json!({ "file": "resources" }));
    }

    #[test]
    fn memory_get_payload_reads_all_files_and_returns_last_50_episodes_newest_first() {
        let base = unique_temp_dir("memory-get");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let memory = ProjectMemory::new(dirs.memory_dir(), "ExampleProject");
        assert!(memory.write("resources", "Switch 1 = boss phase").ok);
        assert!(memory.write("structure", "main.eps: entry point").ok);
        assert!(
            memory
                .write("conventions", "Use dc_ prefix for counters")
                .ok
        );
        assert!(memory.write("lessons", "Build after eps edits").ok);
        for index in 0..55 {
            assert!(memory.append_episode(&json!({
                "ts": format!("2026-06-10T12:{index:02}:00Z"),
                "request_id": format!("req-{index}"),
                "instruction": format!("instruction {index}"),
                "kind": if index % 2 == 0 { "answer" } else { "changeset" },
                "tools": ["file_write"],
                "files": ["main.eps"],
                "decision": if index % 2 == 0 { "answer" } else { "accepted" }
            })));
        }

        let payload = ipc::memory_get_payload(&dirs, "ExampleProject")
            .expect("memory_get should return the open project's memory");

        assert_eq!(payload.project, "ExampleProject");
        assert_eq!(payload.files.resources, "Switch 1 = boss phase");
        assert_eq!(payload.files.structure, "main.eps: entry point");
        assert_eq!(payload.files.conventions, "Use dc_ prefix for counters");
        assert_eq!(payload.files.lessons, "Build after eps edits");
        assert_eq!(payload.episodes.len(), 50);

        let value = serde_json::to_value(&payload).unwrap();
        let episodes = value["episodes"].as_array().unwrap();
        assert_eq!(episodes[0]["request_id"], "req-54");
        assert_eq!(episodes[49]["request_id"], "req-5");

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn memory_save_payload_round_trips_without_journal_and_rejects_invalid_requests() {
        let base = unique_temp_dir("memory-save");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let memory = ProjectMemory::new(dirs.memory_dir(), "ExampleProject");
        assert!(memory.write("resources", "prior").ok);

        let saved = ipc::memory_save_payload(
            &dirs,
            "ExampleProject",
            "resources",
            "Switch 1 = boss phase",
        )
        .expect("memory_save should write a known file for the open project");
        assert_json(&saved, json!({ "file": "resources" }));
        assert_eq!(memory.read("resources"), "Switch 1 = boss phase");
        assert!(
            !dirs.journal_dir().exists(),
            "panel memory_save writes directly and must not create a journal entry"
        );

        let err = ipc::memory_get_payload(&dirs, "   ").unwrap_err();
        assert!(
            err.contains("no project"),
            "empty project should return a no-project error, got {err:?}"
        );

        let err = ipc::memory_save_payload(&dirs, "   ", "resources", "new").unwrap_err();
        assert!(
            err.contains("no project"),
            "empty project should return a no-project error, got {err:?}"
        );

        let err = ipc::memory_save_payload(&dirs, "ExampleProject", "unknown", "new").unwrap_err();
        assert!(
            err.contains("unknown memory file"),
            "unknown file should preserve store validation wording, got {err:?}"
        );
        assert_eq!(memory.read("resources"), "Switch 1 = boss phase");

        let oversize = "x".repeat(CONTENT_CAP_BYTES + 1);
        let err =
            ipc::memory_save_payload(&dirs, "ExampleProject", "resources", &oversize).unwrap_err();
        assert!(
            err.contains(&format!("{CONTENT_CAP_BYTES}-byte budget")),
            "oversize content should preserve store cap wording, got {err:?}"
        );
        assert_eq!(memory.read("resources"), "Switch 1 = boss phase");

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn wiki_get_and_save_payloads_round_trip_and_mark_user_edits() {
        use crate::wiki::{LedgerEntry, WikiStore};
        use std::collections::BTreeMap;

        let base = unique_temp_dir("wiki-payload");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));

        // Seed a ledger via the store (as the accept-hook would).
        let mut store = WikiStore::load(dirs.wiki_dir("ExampleProject"));
        store.upsert(LedgerEntry {
            table: "dat".to_string(),
            dat: "units".to_string(),
            obj_id: 0,
            property: "HP".to_string(),
            value: json!(80),
            item_name: Some("Terran Marine".to_string()),
            applied_at: 1_718_000_000,
            edited_by_user: false,
        });
        store.save().unwrap();

        let got = ipc::wiki_get_payload(&dirs, "ExampleProject").expect("wiki_get");
        assert_eq!(got.version, 1);
        assert_eq!(got.entries["dat:units:0:HP"].value, json!(80));
        assert!(!got.entries["dat:units:0:HP"].edited_by_user);

        // wiki_save flips editedByUser=true and persists the correction.
        let mut entries: BTreeMap<String, LedgerEntry> = BTreeMap::new();
        let mut corrected = got.entries["dat:units:0:HP"].clone();
        corrected.value = json!(100);
        entries.insert(corrected.key(), corrected);

        let saved = ipc::wiki_save_payload(&dirs, "ExampleProject", entries).expect("wiki_save");
        assert_eq!(saved.entries["dat:units:0:HP"].value, json!(100));
        assert!(saved.entries["dat:units:0:HP"].edited_by_user);

        // Persisted to disk.
        let reloaded = WikiStore::load(dirs.wiki_dir("ExampleProject"));
        assert_eq!(
            reloaded.ledger().entries["dat:units:0:HP"].value,
            json!(100)
        );

        // Empty project name disables both commands.
        assert!(ipc::wiki_get_payload(&dirs, "   ")
            .unwrap_err()
            .contains("no project"));
        assert!(ipc::wiki_save_payload(&dirs, "   ", BTreeMap::new())
            .unwrap_err()
            .contains("no project"));

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn wiki_response_serializes_with_camelcase_entry_fields() {
        use crate::wiki::{Ledger, LedgerEntry};

        let mut ledger = Ledger::default();
        ledger.upsert(LedgerEntry {
            table: "dat".to_string(),
            dat: "units".to_string(),
            obj_id: 0,
            property: "HP".to_string(),
            value: json!(80),
            item_name: Some("Terran Marine".to_string()),
            applied_at: 1_718_000_000,
            edited_by_user: false,
        });
        let response = ipc::WikiResponse::from(&ledger);
        assert_json(
            &response,
            json!({
                "version": 1,
                "entries": {
                    "dat:units:0:HP": {
                        "table": "dat",
                        "dat": "units",
                        "objId": 0,
                        "property": "HP",
                        "value": 80,
                        "itemName": "Terran Marine",
                        "appliedAt": 1_718_000_000,
                        "editedByUser": false
                    }
                }
            }),
        );
    }

    #[test]
    fn bridge_state_from_config_rejects_unset_editor_path() {
        let base = unique_temp_dir("unset-editor");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.save_config(&Config::default()).unwrap();

        let err = ipc::bridge_from_config(&dirs).unwrap_err();

        // First-run signal, distinct from the stale-heartbeat "editor not
        // connected" — the panel routes it to the setup screen.
        assert_eq!(err, "editor path not configured");

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

    #[test]
    fn resolve_editor_exe_maps_unset_missing_and_present() {
        // Unset editor path -> the first-run code (panel routes to setup).
        assert_eq!(
            ipc::resolve_editor_exe("   ").unwrap_err(),
            "editor path not configured"
        );

        // Configured path but the exe is absent (moved/renamed install).
        let base = unique_temp_dir("launch-editor");
        assert_eq!(
            ipc::resolve_editor_exe(&base.to_string_lossy()).unwrap_err(),
            "editor executable not found"
        );

        // The exe present at the install root resolves to its absolute path.
        let exe = base.join("EUD Editor 3.exe");
        fs::write(&exe, b"stub").unwrap();
        assert_eq!(
            ipc::resolve_editor_exe(&base.to_string_lossy()).unwrap(),
            exe
        );

        fs::remove_dir_all(&base).ok();
    }
}
