//! The commands a remote client may invoke.
//!
//! This is an explicit allowlist, never the desktop's whole command list: a
//! phone gets the main window's EPS chat flow, the Map window's view, chat
//! and Apply/Undo, opens and closes the PC's recent projects, and nothing
//! that opens native pickers, changes paths or settings, imports, updates,
//! launches programs, signs in to providers, edits on the Map canvas, saves
//! map properties, or manages remote access itself. Every entry
//! deserializes its arguments as the Tauri command does (camelCase) and calls
//! the same `*_inner` function the Tauri command wraps, with the connection's
//! virtual label where the desktop passes a window label. A Map command acts
//! as [`MapCaller::Remote`], which resolves the attached project exactly as
//! the desktop Map window resolves its own.

use std::future::Future;
use std::pin::Pin;

use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Manager;

use super::caller::RemoteCaller;
use crate::engine::SessionEngineManager;
use crate::map_agent::{MapAgentService, MapCaller};
use crate::project_id::ProjectId;
use crate::project_registry::ProjectRegistry;

/// Commands answered by the remote server itself (connection state).
pub const SERVER_COMMANDS: &[&str] = &[
    "remote_projects",
    "remote_recent_list",
    "remote_attach",
    "remote_detach",
];

/// Commands forwarded to the app, each with the desktop command's name.
pub const DISPATCHED_COMMANDS: &[&str] = &[
    "status",
    "list",
    "skill_list",
    "session_list",
    "session_load",
    "session_open",
    "session_create",
    "session_rename",
    "session_delete",
    "session_update_log",
    "ask_pending",
    "chat",
    "chat_steer",
    "plan_feedback",
    "plan_approve",
    "ask_response",
    "cancel",
    "compact",
    "conversation_rewind",
    "mention_search",
    "attachment_discard",
    "provider_status_list",
    "provider_settings",
    "provider_catalog",
    "session_model_settings",
    "session_model_settings_save",
    "project_build_run",
    // Opened or closed here, then attached or detached by the server.
    "remote_open_project",
    "remote_close_project",
    // The Map window's sessions, view, chat and Apply/Undo. Canvas tools
    // (image/stamp placement), selections, properties, image export and
    // window commands stay on the PC.
    "map_agent_bootstrap",
    "map_agent_session_list",
    "map_agent_session_create",
    "map_agent_session_load",
    "map_agent_source_state",
    "map_agent_candidate_state",
    "map_agent_chat",
    "map_agent_chat_steer",
    "map_agent_cancel",
    "map_agent_run_snapshot",
    "map_agent_candidate_apply",
    "map_agent_apply_undo",
    "map_agent_candidate_discard",
    "map_agent_candidate_revert",
    "map_agent_conversation_reset",
    "map_agent_conversation_rewind",
];

/// Commands whose result is bytes (`application/octet-stream`), as the
/// desktop's `tauri::ipc::Response`.
pub const BINARY_COMMANDS: &[&str] = &["map_agent_render"];

pub const NOT_ALLOWED: &str = "원격 접속에서는 사용할 수 없는 기능입니다. PC에서 진행해 주세요.";
pub const NOT_ATTACHED: &str = "프로젝트를 먼저 선택해 주세요.";
pub const SESSION_NOT_HERE: &str =
    "이 대화는 선택한 프로젝트에 속하지 않습니다. 대화 목록을 다시 불러와 주세요.";

/// Whether a remote client may invoke `command`.
pub fn is_allowed(command: &str) -> bool {
    SERVER_COMMANDS.contains(&command)
        || DISPATCHED_COMMANDS.contains(&command)
        || is_binary(command)
}

/// Whether `command` answers with bytes rather than JSON.
pub fn is_binary(command: &str) -> bool {
    BINARY_COMMANDS.contains(&command)
}

/// One remote command invocation.
#[derive(Debug, Clone)]
pub struct RemoteCall {
    pub caller: RemoteCaller,
    /// The project the connection is attached to, if any.
    pub attached: Option<ProjectId>,
    pub command: String,
    pub args: Value,
}

pub type DispatchFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
pub type BinaryFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send>>;

/// Runs allowlisted commands against the app. The server checks the
/// allowlist before calling it; a dispatcher refuses anything else anyway.
pub trait RemoteDispatcher: Send + Sync {
    fn invoke(&self, call: RemoteCall) -> DispatchFuture;
    /// Run one of [`BINARY_COMMANDS`].
    fn invoke_binary(&self, call: RemoteCall) -> BinaryFuture;
    /// Stage one attachment upload (headers and raw body as the desktop sends).
    fn stage_attachment(&self, headers: HeaderMap, body: Bytes) -> DispatchFuture;
}

/// The product dispatcher: the app's managed state.
pub struct TauriDispatcher {
    app: tauri::AppHandle,
}

impl TauriDispatcher {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl RemoteDispatcher for TauriDispatcher {
    fn invoke(&self, call: RemoteCall) -> DispatchFuture {
        let app = self.app.clone();
        Box::pin(async move { run(&app, call).await })
    }

    fn invoke_binary(&self, call: RemoteCall) -> BinaryFuture {
        let app = self.app.clone();
        Box::pin(async move { run_binary(&app, call).await })
    }

    fn stage_attachment(&self, headers: HeaderMap, body: Bytes) -> DispatchFuture {
        let store = self
            .app
            .state::<crate::attachment::AttachmentManaged>()
            .store()
            .clone();
        Box::pin(async move {
            let staged = tokio::task::spawn_blocking(move || {
                crate::attachment::stage_attachment_inner(&store, &headers, &body)
            })
            .await
            .map_err(|error| error.to_string())?;
            json(staged)
        })
    }
}

fn json<T: Serialize>(result: Result<T, String>) -> Result<Value, String> {
    serde_json::to_value(result?).map_err(|error| error.to_string())
}

fn args<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("요청 인자가 올바르지 않습니다: {error}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionArgs {
    session_id: String,
}

#[derive(Deserialize)]
struct IdArgs {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateArgs {
    first_text: String,
}

#[derive(Deserialize)]
struct RenameArgs {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateLogArgs {
    id: String,
    panel_log: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RewindArgs {
    session_id: String,
    panel_log: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskResponseArgs {
    session_id: String,
    request_id: String,
    answers: std::collections::BTreeMap<String, crate::ipc::AskAnswer>,
}

#[derive(Deserialize)]
struct MentionArgs {
    request: crate::mentions::MentionSearchRequest,
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseProjectArgs {
    project_id: String,
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
struct ProviderArgs {
    provider: crate::provider::ProviderId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertArgs {
    session_id: String,
    revision: u32,
}

#[derive(Deserialize)]
struct CommandArgs<T> {
    command: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelSaveArgs {
    session_id: String,
    model: String,
    reasoning: Option<crate::provider::ReasoningSelection>,
}

/// A session command is admitted only for a session of the attached project:
/// a phone never reaches a conversation of a project it did not select.
fn own_session(
    engine: &SessionEngineManager,
    attached: Option<&ProjectId>,
    session_id: &str,
) -> Result<(), String> {
    let attached = attached.ok_or_else(|| NOT_ATTACHED.to_string())?;
    match engine.session_project_id(session_id) {
        Some(project) if &project == attached => Ok(()),
        _ => Err(SESSION_NOT_HERE.to_string()),
    }
}

async fn run(app: &tauri::AppHandle, call: RemoteCall) -> Result<Value, String> {
    let RemoteCall {
        caller,
        attached,
        command,
        args: raw,
    } = call;
    let label = caller.label.as_str();
    let attached = attached.as_ref();
    let engine = app.state::<SessionEngineManager>();
    let registry = app.state::<ProjectRegistry>();
    use crate::engine as e;
    match command.as_str() {
        "status" => json(crate::ipc::status_inner(&registry, label).await),
        "list" => json(crate::ipc::list_inner(&registry, label).await),
        "skill_list" => json(Ok(crate::skills::skill_list())),
        "session_list" => json(e::session_list_inner(&engine, label).await),
        "session_create" => {
            let CreateArgs { first_text } = args(raw)?;
            json(e::session_create_inner(&engine, label, &first_text))
        }
        "session_load" => {
            let IdArgs { id } = args(raw)?;
            own_session(&engine, attached, &id)?;
            json(e::session_load_inner(&engine, &id))
        }
        "session_open" => {
            let IdArgs { id } = args(raw)?;
            own_session(&engine, attached, &id)?;
            json(e::session_open_inner(&engine, &id).await)
        }
        "session_rename" => {
            let RenameArgs { id, name } = args(raw)?;
            own_session(&engine, attached, &id)?;
            json(e::session_rename_inner(&engine, &id, &name))
        }
        "session_delete" => {
            let IdArgs { id } = args(raw)?;
            own_session(&engine, attached, &id)?;
            json(e::session_delete_inner(&engine, &id).await)
        }
        "session_update_log" => {
            let UpdateLogArgs { id, panel_log } = args(raw)?;
            own_session(&engine, attached, &id)?;
            json(e::session_update_log_inner(&engine, &id, panel_log))
        }
        "ask_pending" => {
            let SessionArgs { session_id } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::ask_pending_inner(&engine, &session_id).await)
        }
        "chat" => {
            let command: e::ChatCommand = args(raw)?;
            own_session(&engine, attached, &command.session_id)?;
            json(e::chat_inner(&engine, command).await)
        }
        "chat_steer" => {
            let command: e::steer::ChatSteerCommand = args(raw)?;
            own_session(&engine, attached, &command.session_id)?;
            json(e::steer::chat_steer_inner(&engine, command).await)
        }
        "plan_feedback" => {
            let command: e::PlanFeedbackCommand = args(raw)?;
            own_session(&engine, attached, &command.session_id)?;
            json(e::plan_feedback_inner(&engine, command).await)
        }
        "plan_approve" => {
            let SessionArgs { session_id } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::plan_approve_inner(&engine, &session_id).await)
        }
        "ask_response" => {
            let AskResponseArgs {
                session_id,
                request_id,
                answers,
            } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::ask_response_inner(&engine, &session_id, request_id, answers).await)
        }
        "cancel" => {
            let SessionArgs { session_id } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::cancel_inner(&engine, &session_id).await)
        }
        "compact" => {
            let SessionArgs { session_id } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::compact_inner(&engine, &session_id).await)
        }
        "conversation_rewind" => {
            let RewindArgs {
                session_id,
                panel_log,
            } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::conversation_rewind_inner(&engine, &session_id, panel_log).await)
        }
        "session_model_settings" => {
            let SessionArgs { session_id } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::session_model_settings_inner(&engine, &session_id).await)
        }
        "session_model_settings_save" => {
            let ModelSaveArgs {
                session_id,
                model,
                reasoning,
            } = args(raw)?;
            own_session(&engine, attached, &session_id)?;
            json(e::session_model_settings_save_inner(&engine, &session_id, model, reasoning).await)
        }
        "mention_search" => {
            let MentionArgs { request } = args(raw)?;
            let service = app.state::<crate::mentions::MentionService>();
            json(crate::mentions::mention_search_inner(&service, &registry, label, request).await)
        }
        "attachment_discard" => {
            let IdArgs { id } = args(raw)?;
            let store = app
                .state::<crate::attachment::AttachmentManaged>()
                .store()
                .clone();
            json(
                tokio::task::spawn_blocking(move || store.discard_draft(&id))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            )
        }
        "provider_status_list" => {
            let service = app.state::<crate::provider_service::ProviderService>();
            json(service.status_list().await)
        }
        // Read-only: the defaults a new conversation starts with. The saved
        // endpoint stays on the PC; secrets are only ever a presence flag.
        "provider_settings" => {
            let ProviderArgs { provider } = args(raw)?;
            let service = app.state::<crate::provider_service::ProviderService>();
            let mut view = service.settings_view(provider).await?;
            view.base_url = None;
            json(Ok(view))
        }
        "remote_open_project" => {
            let PathArgs { path } = args(raw)?;
            let dirs = registry.dirs().clone();
            let engine = engine.inner().clone();
            let opened = tokio::task::spawn_blocking(move || {
                let config = dirs.load_config().map_err(|error| error.to_string())?;
                let path = super::caller::recent_path(&config, &path)?;
                let setup = crate::native_runtime::NativeProjectSetup::new(dirs);
                engine.open_for_remote(std::path::Path::new(&path), || {
                    setup
                        .configure_project(std::path::Path::new(&path))
                        .map(|(project, _)| project)
                        .map_err(|error| format!("프로젝트를 열지 못했습니다: {error}"))
                })
            })
            .await
            .map_err(|error| error.to_string())??;
            json(Ok(serde_json::json!({ "projectId": opened.id().as_str() })))
        }
        "remote_close_project" => {
            let CloseProjectArgs { project_id, force } = args(raw)?;
            let engine = engine.inner().clone();
            let closed = tokio::task::spawn_blocking(move || {
                engine.close_for_remote(&ProjectId::from_stored(&project_id), force)
            })
            .await
            .map_err(|error| error.to_string())??;
            json(Ok(serde_json::json!({
                "closed": closed == crate::engine::RemoteClose::Closed,
            })))
        }
        "provider_catalog" => {
            let ProviderArgs { provider } = args(raw)?;
            let service = app.state::<crate::provider_service::ProviderService>();
            json(service.catalog(provider).await)
        }
        "project_build_run" => json(crate::setup::project_build_run_inner(&registry, label).await),
        command if command.starts_with("map_agent_") => run_map(app, label, command, raw).await,
        _ => Err(NOT_ALLOWED.to_string()),
    }
}

/// The Map window's commands, as the attached project's remote Map caller.
/// Every session command first checks the session is a Map session of the
/// attached project, the check the desktop Map window makes for its own.
async fn run_map(
    app: &tauri::AppHandle,
    label: &str,
    command: &str,
    raw: Value,
) -> Result<Value, String> {
    use crate::map_agent as m;
    let caller = MapCaller::Remote(label);
    let service = app.state::<MapAgentService>();
    let engines = app.state::<SessionEngineManager>();
    match command {
        "map_agent_bootstrap" => json(m::map_bootstrap_inner(&service, &engines, caller).await),
        "map_agent_session_list" => json(m::map_session_list_inner(&service, caller)),
        "map_agent_session_create" => {
            json(m::map_session_create_inner(&service, &engines, caller).await)
        }
        "map_agent_session_load" => {
            let SessionArgs { session_id } = args(raw)?;
            json(m::map_session_load_inner(&service, &engines, caller, &session_id).await)
        }
        "map_agent_source_state" => json(m::map_source_state_inner(&service, caller).await),
        "map_agent_candidate_state" => {
            let SessionArgs { session_id } = args(raw)?;
            service.caller_session(caller, &session_id)?;
            json(m::map_candidate_state_inner(&service, &session_id))
        }
        "map_agent_chat" => {
            let CommandArgs { command } = args::<CommandArgs<m::MapChatCommand>>(raw)?;
            json(m::map_chat_inner(app, &service, &engines, caller, command).await)
        }
        "map_agent_chat_steer" => {
            let CommandArgs { command } = args::<CommandArgs<m::MapChatSteerCommand>>(raw)?;
            json(m::map_chat_steer_inner(&service, &engines, caller, command).await)
        }
        "map_agent_cancel" => {
            let SessionArgs { session_id } = args(raw)?;
            json(m::map_cancel_inner(&service, &engines, caller, &session_id).await)
        }
        "map_agent_run_snapshot" => {
            let SessionArgs { session_id } = args(raw)?;
            json(m::map_run_snapshot_inner(&service, &engines, caller, &session_id).await)
        }
        // Apply and Undo run the Map window's own path (the project's
        // `map:<id>` transaction and MapSafe) on a blocking thread, so a
        // refusal such as SCMDraft holding the map reaches the phone as is.
        "map_agent_candidate_apply" | "map_agent_apply_undo" => {
            let SessionArgs { session_id } = args(raw)?;
            let app = app.clone();
            let label = label.to_string();
            let apply = command == "map_agent_candidate_apply";
            json(
                tokio::task::spawn_blocking(move || {
                    let service = app.state::<MapAgentService>();
                    let caller = MapCaller::Remote(&label);
                    if apply {
                        m::map_candidate_apply_inner(&app, &service, caller, &session_id)
                    } else {
                        m::map_apply_undo_inner(&app, &service, caller, &session_id)
                    }
                })
                .await
                .map_err(|error| error.to_string())?,
            )
        }
        "map_agent_candidate_discard" => {
            let SessionArgs { session_id } = args(raw)?;
            service.caller_session(caller, &session_id)?;
            json(m::map_candidate_discard_inner(app, &service, &session_id))
        }
        "map_agent_candidate_revert" => {
            let RevertArgs {
                session_id,
                revision,
            } = args(raw)?;
            service.caller_session(caller, &session_id)?;
            json(m::map_candidate_revert_inner(
                app,
                &service,
                &session_id,
                revision,
            ))
        }
        "map_agent_conversation_reset" => {
            let SessionArgs { session_id } = args(raw)?;
            json(m::map_conversation_reset_inner(&service, &engines, caller, &session_id).await)
        }
        "map_agent_conversation_rewind" => {
            let RewindArgs {
                session_id,
                panel_log,
            } = args(raw)?;
            json(
                m::map_conversation_rewind_inner(
                    &service,
                    &engines,
                    caller,
                    &session_id,
                    panel_log,
                )
                .await,
            )
        }
        _ => Err(NOT_ALLOWED.to_string()),
    }
}

async fn run_binary(app: &tauri::AppHandle, call: RemoteCall) -> Result<Vec<u8>, String> {
    let RemoteCall {
        caller,
        command,
        args: raw,
        ..
    } = call;
    let service = app.state::<MapAgentService>();
    match command.as_str() {
        "map_agent_render" => {
            let CommandArgs { command } =
                args::<CommandArgs<crate::map_agent::MapRenderCommand>>(raw)?;
            service.caller_session(MapCaller::Remote(&caller.label), &command.session_id)?;
            crate::map_agent::map_render_inner(&service, command).await
        }
        _ => Err(NOT_ALLOWED.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_is_the_eps_chat_flow_only() {
        for command in DISPATCHED_COMMANDS.iter().chain(SERVER_COMMANDS) {
            assert!(is_allowed(command), "{command}");
        }
        // Native pickers, settings and paths, imports, updates, launches,
        // provider sign-in, remote management, Map, and unknown names.
        for command in [
            "setup_pick_project_path",
            "setup_pick_euddraft_path",
            "setup_import_e3s",
            "setup_import_e2",
            "project_open",
            "project_recent_remove",
            "project_open_scmdraft",
            "project_open_root_in",
            "project_open_new_window",
            "app_settings_save",
            "euddraft_update",
            "provider_login_start",
            "provider_api_key_save",
            "provider_logout",
            "provider_install",
            "provider_defaults_save",
            "remote_pair_begin",
            "remote_settings_save",
            "remote_device_revoke",
            "remote_tailscale_serve",
            "workspace_read_bytes",
            "memory_save",
            "git_revert",
            "autonomous_resume",
            "attachment_stage",
            "browser_state",
            "browser_activate",
            "browser_tab_close",
            "browser_user_navigate",
            "browser_chrome_expand",
            "browser_profile_reset",
            "plugin:event|listen",
            "",
            "Chat",
        ] {
            assert!(!is_allowed(command), "{command}");
        }
    }

    #[test]
    fn the_map_allowlist_is_view_chat_and_apply_only() {
        for command in [
            "map_agent_bootstrap",
            "map_agent_session_list",
            "map_agent_session_create",
            "map_agent_session_load",
            "map_agent_source_state",
            "map_agent_candidate_state",
            "map_agent_chat",
            "map_agent_chat_steer",
            "map_agent_cancel",
            "map_agent_run_snapshot",
            "map_agent_candidate_apply",
            "map_agent_apply_undo",
            "map_agent_candidate_discard",
            "map_agent_candidate_revert",
            "map_agent_conversation_reset",
            "map_agent_conversation_rewind",
            "map_agent_render",
            // Shared with EPS sessions and already allowlisted.
            "compact",
            "session_model_settings",
            "session_model_settings_save",
            "ask_pending",
            "ask_response",
            "session_update_log",
        ] {
            assert!(is_allowed(command), "{command}");
        }
        assert!(is_binary("map_agent_render"));
        assert!(!is_binary("map_agent_candidate_state"));
        assert!(!DISPATCHED_COMMANDS.contains(&"map_agent_render"));
        // Desktop canvas tools, native pickers, properties, windows, imports.
        for command in [
            "map_agent_open",
            "map_agent_open_properties",
            "map_agent_properties_save",
            "map_agent_export_image",
            "map_agent_export_image_save",
            "map_agent_thumbnail",
            "map_agent_image_preview",
            "map_agent_image_confirm",
            "map_agent_image_cancel",
            "map_agent_stamp_preview",
            "map_agent_stamp_confirm",
            "map_agent_catalog",
            "map_agent_objects",
            "map_agent_diff_details",
            "map_agent_selection_save",
            "map_agent_selection_delete",
            "map_agent_session_rename",
            "map_agent_session_delete",
            "map_agent_focus_main",
            "map_task_apply_undo",
            "map_agent_import_open",
            "map_import_stamp_list",
        ] {
            assert!(!is_allowed(command), "{command}");
        }
    }

    #[test]
    fn map_command_arguments_are_read_as_the_desktop_sends_them() {
        let chat: CommandArgs<crate::map_agent::MapChatCommand> = args(serde_json::json!({
            "command": {
                "sessionId": "m1",
                "text": "언덕",
                "attachments": [],
                "candidateRevision": 2,
                "mentions": [],
            }
        }))
        .unwrap();
        assert_eq!(chat.command.session_id, "m1");
        assert_eq!(chat.command.candidate_revision, 2);
        assert!(!chat.command.image_placement);
        let render: CommandArgs<crate::map_agent::MapRenderCommand> = args(serde_json::json!({
            "command": {
                "sessionId": "m1",
                "view": "candidate",
                "x": 0, "y": 0, "width": 64, "height": 64, "scale": 8,
                "layers": ["terrain"],
            }
        }))
        .unwrap();
        assert_eq!(render.command.session_id, "m1");
        let revert: RevertArgs =
            args(serde_json::json!({"sessionId": "m1", "revision": 1})).unwrap();
        assert_eq!(revert.revision, 1);
    }

    #[test]
    fn command_arguments_are_read_as_the_desktop_sends_them() {
        let chat: crate::engine::ChatCommand = args(serde_json::json!({
            "sessionId": "s1",
            "clientTurnId": "t1",
            "text": "안녕",
            "attachments": ["a1"],
            "clarify": true,
            "skillSelections": {"release": true},
        }))
        .unwrap();
        assert_eq!(chat.session_id, "s1");
        assert_eq!(chat.client_turn_id, "t1");
        assert_eq!(chat.attachments, ["a1"]);
        assert_eq!(chat.clarify, Some(true));
        assert!(chat.mentions.is_none());

        let error = args::<SessionArgs>(serde_json::json!({"session_id": "s"})).unwrap_err();
        assert!(error.contains("sessionId"), "{error}");
        let save: ModelSaveArgs =
            args(serde_json::json!({"sessionId": "s", "model": "m"})).unwrap();
        assert!(save.reasoning.is_none());
    }

    /// A phone steers a running turn the way the desktop does; a resent steer
    /// is answered from the ledger, never pushed twice.
    #[test]
    fn steering_is_allowed_remembered_and_read_as_the_desktop_sends_it() {
        for command in ["chat_steer", "map_agent_chat_steer"] {
            assert!(is_allowed(command), "{command}");
            assert!(
                super::super::server::REMEMBERED_COMMANDS.contains(&command),
                "{command}"
            );
        }
        let eps: crate::engine::steer::ChatSteerCommand = args(serde_json::json!({
            "sessionId": "s",
            "steerId": "8d1c8f4e-6c0e-4b52-9a51-2f6f2b7d3e10",
            "text": "빨간색으로",
            "attachments": [],
        }))
        .unwrap();
        assert_eq!(eps.text, "빨간색으로");
        assert!(eps.mentions.is_none());
        let map: CommandArgs<crate::map_agent::MapChatSteerCommand> = args(serde_json::json!({
            "command": {
                "sessionId": "m1",
                "steerId": "8d1c8f4e-6c0e-4b52-9a51-2f6f2b7d3e10",
                "text": "더 높게",
                "attachments": [],
            }
        }))
        .unwrap();
        assert!(map.command.mentions.is_empty());
    }
}
