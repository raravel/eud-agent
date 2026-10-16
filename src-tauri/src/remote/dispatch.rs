//! The commands a remote client may invoke.
//!
//! This is an explicit allowlist, never the desktop's whole command list: a
//! phone gets the main window's EPS chat flow and nothing that opens native
//! pickers, changes paths or settings, imports, updates, launches programs,
//! signs in to providers, or manages remote access itself. Every entry
//! deserializes its arguments as the Tauri command does (camelCase) and calls
//! the same `*_inner` function the Tauri command wraps, with the connection's
//! virtual label where the desktop passes a window label.

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
use crate::project_id::ProjectId;
use crate::project_registry::ProjectRegistry;

/// Commands answered by the remote server itself (connection state).
pub const SERVER_COMMANDS: &[&str] = &["remote_projects", "remote_attach", "remote_detach"];

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
    "plan_feedback",
    "plan_approve",
    "ask_response",
    "cancel",
    "compact",
    "conversation_rewind",
    "mention_search",
    "attachment_discard",
    "provider_status_list",
    "provider_catalog",
    "session_model_settings",
    "session_model_settings_save",
    "project_build_run",
];

pub const NOT_ALLOWED: &str = "원격 접속에서는 사용할 수 없는 기능입니다. PC에서 진행해 주세요.";
pub const NOT_ATTACHED: &str = "프로젝트를 먼저 선택해 주세요.";
pub const SESSION_NOT_HERE: &str =
    "이 대화는 선택한 프로젝트에 속하지 않습니다. 대화 목록을 다시 불러와 주세요.";

/// Whether a remote client may invoke `command`.
pub fn is_allowed(command: &str) -> bool {
    SERVER_COMMANDS.contains(&command) || DISPATCHED_COMMANDS.contains(&command)
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

/// Runs allowlisted commands against the app. The server checks the
/// allowlist before calling it; a dispatcher refuses anything else anyway.
pub trait RemoteDispatcher: Send + Sync {
    fn invoke(&self, call: RemoteCall) -> DispatchFuture;
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
struct ProviderArgs {
    provider: crate::provider::ProviderId,
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
        "provider_catalog" => {
            let ProviderArgs { provider } = args(raw)?;
            let service = app.state::<crate::provider_service::ProviderService>();
            json(service.catalog(provider).await)
        }
        "project_build_run" => json(crate::setup::project_build_run_inner(&registry, label).await),
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
            "map_agent_chat",
            "map_agent_candidate_apply",
            "autonomous_resume",
            "attachment_stage",
            "plugin:event|listen",
            "",
            "Chat",
        ] {
            assert!(!is_allowed(command), "{command}");
        }
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
}
