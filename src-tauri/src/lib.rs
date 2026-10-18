//! eud-agent Tauri 2 application shell.
//!
//! Native standalone desktop shell. The Tauri builder hosts the React panel,
//! native project/runtime services, euddraft generation, map tooling, Codex,
//! RAG, memory, and typed in-process IPC.

use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager};

pub mod antigravity_auth;
pub mod antigravity_client;
pub mod attachment;
pub mod audio;
pub mod autonomous;
pub mod blank_project;
pub mod bootstrap;
pub mod chk;
pub mod claude_auth;
pub mod claude_client;
pub mod codex_adapter;
pub mod codex_auth;
pub mod codex_client;
pub mod config;
pub mod context_state;
pub mod dat_wiki;
pub mod e2_import;
pub mod e3s_nrbf;
pub mod engine;
pub mod fs_tools;
pub mod git;
pub mod grp;
pub mod harness_import;
pub mod ipc;
pub mod iscript;
pub mod jev;
pub mod journal;
pub mod map_agent;
pub mod map_candidate;
pub mod map_context;
pub mod map_image;
pub mod map_import;
pub mod map_model;
pub mod map_render_grid;
pub mod map_stamp;
pub mod map_verify;
pub mod mapsafe;
pub mod mcp;
pub mod memory;
pub mod mentions;
pub mod native_build;
pub mod native_project;
pub mod native_runtime;
mod nrbf;
pub mod ollama;
pub mod opencode_go;
pub mod project_events;
pub mod project_id;
pub mod project_launch;
pub(crate) mod project_rebind;
pub mod project_registry;
pub mod provider;
pub(crate) mod provider_process;
pub mod provider_runtime;
pub mod provider_secrets;
pub mod provider_service;
pub mod provider_tool_loop;
pub mod provider_transcript;
pub mod rag;
pub mod rag_panel;
pub mod release;
pub mod remote;
pub mod session;
pub(crate) mod session_binding;
pub mod session_search;
pub mod setup;
pub mod skills;
pub mod source_snapshot;
pub mod steer;
pub mod task_state;
pub mod team;
pub mod tool_exec;
pub mod tools;
pub mod trace_test;
pub mod wiki;
#[cfg(windows)]
pub mod windows_notification;
mod work_checkpoint;
pub mod worker;
pub mod workspace;
pub mod write_coordinator;

fn finish_background_ffmpeg_bootstrap(
    emitter: &(dyn bootstrap::ProgressEmitter + Send + Sync),
    result: anyhow::Result<bootstrap::ManagedFfmpegPaths>,
) {
    if let Err(error) = result {
        eprintln!("eud-agent: managed audio converter remains unavailable: {error}");
        emitter.emit(
            "bootstrap",
            100,
            "audio converter unavailable; chat and existing attachments remain available",
        );
    }
    emitter.emit("bootstrap", 100, "done");
}

/// A Finder-launched macOS app inherits launchd's `/usr/bin:/bin:/usr/sbin:/sbin`, so
/// user-installed CLIs (`~/.local/bin/claude`, `codex`, Homebrew) are invisible to
/// `PATH` lookup. Adopt the login shell's `PATH` before any thread starts; a shell that
/// fails or stalls leaves the inherited value untouched.
#[cfg(target_os = "macos")]
fn adopt_login_shell_path() {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const MARKER: &str = "__EUD_AGENT_PATH__";
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let Ok(mut child) = Command::new(shell)
        .arg("-ilc")
        .arg(format!("printf '{MARKER}%s{MARKER}' \"$PATH\""))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
    let mut output = String::new();
    let Some(mut stdout) = child.stdout.take() else {
        return;
    };
    if stdout.read_to_string(&mut output).is_err() {
        return;
    }
    let path = output
        .split(MARKER)
        .nth(1)
        .map(str::trim)
        .filter(|path| !path.is_empty());
    if let Some(path) = path {
        std::env::set_var("PATH", path);
    }
}

/// Build and run the Tauri application.
///
/// Close the Map and import windows among `labels`, the windows a closed
/// project had.
fn close_companions(app: &tauri::AppHandle, labels: &[String]) {
    for label in labels.iter().filter(|label| {
        label.starts_with(project_registry::MAP_WINDOW_PREFIX)
            || label.starts_with(project_registry::IMPORT_WINDOW_PREFIX)
    }) {
        if let Some(window) = app.get_webview_window(label) {
            if let Err(error) = window.close() {
                eprintln!("eud-agent: {label} could not be closed with its project: {error}");
            }
        }
    }
}

/// Close `window` the user confirmed closing although its project had work:
/// the work is cancelled, the project and its companions close, and the
/// window goes without asking again.
fn force_close_window(window: &tauri::Window) {
    if let Some(engine) = window.try_state::<engine::SessionEngineManager>() {
        if let Some((_, labels)) = engine.force_close_window(window.label()) {
            close_companions(window.app_handle(), &labels);
        }
    }
    if let Err(error) = window.destroy() {
        eprintln!(
            "eud-agent: window {} could not be closed: {error}",
            window.label()
        );
    }
}

/// Kept out of `main.rs` so the same setup is reusable by mobile targets and
/// integration tests (idiomatic Tauri 2 lib/bin split).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    adopt_login_shell_path();
    tauri::Builder::default()
        .manage(project_launch::PendingProjectLaunch::from_args(
            std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()),
            &std::env::current_dir().unwrap_or_default(),
        ))
        .plugin(tauri_plugin_single_instance::init(
            project_launch::receive_launch,
        ))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .on_window_event(|window, event| {
            let label = window.label().to_string();
            match event {
                // Closing a project window closes its project. While the
                // project has work the user is asked first, and a confirmed
                // close cancels that work instead of waiting for it. A
                // project a remote client is attached to stays open with its
                // work, and the window closes without asking. Its Map window
                // and importer close with it; the process exits with its last
                // window unless remote access is on.
                tauri::WindowEvent::CloseRequested { api, .. }
                    if project_launch::is_project_window(&label) =>
                {
                    let Some(engine) = window.try_state::<engine::SessionEngineManager>() else {
                        return;
                    };
                    match engine.close_window(&label) {
                        Ok(Some((_, labels))) => close_companions(window.app_handle(), &labels),
                        Ok(None) => {}
                        Err(message) => {
                            api.prevent_close();
                            use tauri_plugin_dialog::{
                                DialogExt, MessageDialogButtons, MessageDialogKind,
                            };
                            let target = window.clone();
                            window
                                .app_handle()
                                .dialog()
                                .message(format!(
                                    "{message}

그래도 닫으면 진행 중인 작업을 중단하고 창을 닫습니다. 대화와 검토 대기 중인 계획은 프로젝트를 다시 열면 그대로 남아 있습니다."
                                ))
                                .title("창을 닫을까요?")
                                .kind(MessageDialogKind::Warning)
                                .buttons(MessageDialogButtons::OkCancelCustom(
                                    "닫기".to_string(),
                                    "취소".to_string(),
                                ))
                                .show(move |close| {
                                    if close {
                                        force_close_window(&target);
                                    }
                                });
                        }
                    }
                }
                tauri::WindowEvent::Destroyed => {
                    // Requests the window never took go with it.
                    if let Some(pending) =
                        window.try_state::<project_launch::PendingProjectLaunch>()
                    {
                        pending.forget(&label);
                    }
                    // A closed window works on no project any more.
                    let Some(registry) = window.try_state::<project_registry::ProjectRegistry>()
                    else {
                        return;
                    };
                    let Some(project) = registry.unbind_window(&label) else {
                        return;
                    };
                    if label.starts_with(project_registry::MAP_WINDOW_PREFIX) {
                        // The Map Importer is a companion of the Map window.
                        map_import::close_import_window(window.app_handle(), &project);
                    } else if project_launch::is_project_window(&label) {
                        // A project window gone without a close request: its
                        // companions go with it, and so does its project
                        // unless a remote client is attached to it.
                        let labels = registry.window_labels(&project);
                        if !registry.has_remote_observers(&project) {
                            if let Some(engine) = window.try_state::<engine::SessionEngineManager>()
                            {
                                engine.drop_project(&project);
                            }
                        }
                        close_companions(window.app_handle(), &labels);
                    }
                }
                _ => {}
            }
        })
        .setup(|app| {
            isom::assert_abi_version()?;
            let data_dirs = config::DataDirs::resolve(&*app)?;
            // Non-fatal: a failed dir create resurfaces on first write with context.
            if let Err(error) = data_dirs.ensure_dirs() {
                eprintln!("eud-agent: cannot create data dirs: {error}");
            }
            // One-time cutover: legacy per-session document copies and source
            // mirrors are obsolete now that the CLI cwd is the project root.
            if let Err(error) =
                workspace::WorkspaceManager::new(data_dirs.clone()).discard_legacy_session_mirrors()
            {
                eprintln!("eud-agent: legacy session workspace cleanup failed: {error}");
            }
            match app
                .path()
                .resolve("native/eud-editor-compat", BaseDirectory::Resource)
            {
                Ok(source) => {
                    match native_build::sync_compat_assets(&source, &data_dirs.native_assets_dir())
                    {
                        Ok(copied) if copied > 0 => {
                            eprintln!("eud-agent: installed {copied} native compatibility assets")
                        }
                        Ok(_) => {}
                        Err(error) => eprintln!("eud-agent: native asset sync failed: {error}"),
                    }
                }
                Err(error) => {
                    eprintln!("eud-agent: cannot resolve native compatibility assets: {error}")
                }
            }
            let attachment_store = attachment::AttachmentStore::new(data_dirs.attachments_dir());
            attachment_store.cleanup_stale_drafts();

            let provider_service = provider_service::ProviderService::new(data_dirs.clone())
                .map_err(anyhow::Error::msg)?;
            app.manage(provider_service.clone());

            app.manage(ipc::AppManaged::new(data_dirs.clone()));

            // Remote access is off by default; when enabled in config the
            // loopback server starts here, and a bind failure is only a status.
            let remote_server = remote::RemoteServer::new(
                data_dirs.clone(),
                remote::server::tauri_assets(app.handle().clone()),
            );
            app.manage(remote_server.clone());
            // Remote clients receive project events through this hub;
            // `emit_project` publishes into it.
            let remote_hub = remote::hub::RemoteHub::new();
            app.manage(remote_hub.clone());
            app.manage(attachment::AttachmentManaged::new(attachment_store.clone()));

            // Audio tools are process-wide. Project-dependent setup starts only
            // after the launcher explicitly accepts a project in this run.
            let boot_handle = app.handle().clone();
            let boot_dirs = data_dirs.clone();
            tauri::async_runtime::spawn(async move {
                let emitter = bootstrap::TauriEmitter(boot_handle.clone());
                if !bootstrap::managed_ffmpeg_ready(&boot_dirs) {
                    let result = bootstrap::ensure_ffmpeg(&boot_dirs, &emitter).await;
                    finish_background_ffmpeg_bootstrap(&emitter, result);
                }
            });

            let app_handle = app.handle().clone();

            let activity_handle = app_handle.clone();
            let activity_hub = remote_hub.clone();
            // A phone receives a session's activity only while it is attached
            // to that session's project.
            let route_handle = app_handle.clone();
            let remote_activity =
                remote::hub::session_activity_router(remote_hub.clone(), move |session| {
                    route_handle
                        .try_state::<engine::SessionEngineManager>()
                        .and_then(|engine| engine.session_project_id(session))
                });
            let writes = write_coordinator::ProjectWriteCoordinator::new(move |event| {
                if activity_hub.has_listeners() {
                    let _ = remote_activity.send(event.clone());
                }
                if let Err(error) = activity_handle.emit("session_activity", event) {
                    eprintln!("eud-agent: session activity emit failed: {error}");
                }
            });
            match mapsafe::recover_pending_candidate_applies(
                &data_dirs.map_backups_dir(),
                &data_dirs.map_candidates_dir(),
            ) {
                Ok(recovered) if recovered > 0 => {
                    eprintln!(
                        "eud-agent: restored {recovered} interrupted Map Agent Apply transaction(s)"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("eud-agent: pending Map Agent Apply recovery is blocked: {error}");
                }
            }
            // The runtime authority over open projects. Every project-bound
            // service below takes its project from here, never from the
            // configured path.
            let registry = project_registry::ProjectRegistry::new(data_dirs.clone());
            app.manage(registry.clone());
            let imports =
                map_import::MapImportStore::with_registry(data_dirs.clone(), registry.clone());
            if let Err(error) = imports.cleanup_startup() {
                eprintln!("eud-agent: map import cleanup skipped: {error}");
            }
            app.manage(imports.clone());
            let candidates = map_candidate::CandidateStore::with_registry(
                data_dirs.clone(),
                imports,
                registry.clone(),
            );
            if let Err(error) = candidates.cleanup_startup() {
                eprintln!("eud-agent: candidate cache cleanup skipped: {error}");
            }
            let map_service = map_agent::MapAgentService::new(
                data_dirs.clone(),
                candidates.clone(),
                writes.clone(),
                registry.clone(),
            );
            if let Err(error) = map_service.recover_team_tasks() {
                eprintln!("eud-agent: team map task restart recovery failed: {error}");
            }
            app.manage(map_service);
            let services = tool_exec::ToolServices::new(
                data_dirs.clone(),
                candidates,
                writes,
                registry.clone(),
            );
            app.manage(services.mentions());

            // Warm the RAG embedding model in the background; readiness NEVER
            // gates startup (search_docs returns zero hits until it is ready,
            // which still lifts the evidence gate).
            let warm_rag = services.rag();
            app.manage(rag_panel::RagManaged(services.rag()));
            let warm_handle = app_handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(error) = warm_rag.warmup(&bootstrap::TauriEmitter(warm_handle)) {
                    eprintln!("eud-agent: RAG warmup skipped: {error}");
                }
            });

            // Session restore: Rust owns every session file under
            // `%appdata%\eud-agent\sessions\` (decision A/D). Built before the
            // config builder moves `data_dirs` into the project-state provider.
            let session_store = session::SessionStore::new(&data_dirs);
            if let Err(error) = session_store.recover_interrupted_autonomous_runs() {
                eprintln!("eud-agent: autonomous restart recovery failed: {error}");
            }
            if let Err(error) = session_store.recover_interrupted_workers() {
                eprintln!("eud-agent: worker restart recovery failed: {error}");
            }
            // Extract every conversation's searchable text in the background so
            // the sidebar's first content search reads no session record.
            let session_search =
                std::sync::Arc::new(session_search::SessionSearchIndex::new(&data_dirs));
            app.manage(session_search::SessionSearchManaged::new(
                session_search.clone(),
                session_store.clone(),
            ));
            tauri::async_runtime::spawn_blocking(move || session_search.warm_up());

            // Each engine reads memory, wiki, project state, and the project
            // map from its own session's project handle.
            let config =
                engine::AgentEngineConfig::new("[project state]\n(unavailable)", None, Vec::new())
                    .with_project_providers();

            app.manage(engine::SessionEngineManager::new(
                session_store,
                attachment_store,
                services,
                provider_service,
                config,
                app_handle.clone(),
                data_dirs.clone(),
                data_dirs.codex_workspace_dir(),
                registry.clone(),
            ));
            // Every service a remote command reaches now exists: connect the
            // remote API, then start the server when it is enabled.
            remote_server.set_backend(remote::server::RemoteBackend {
                connections: remote::caller::RemoteConnections::new(registry),
                hub: remote_hub,
                dispatcher: std::sync::Arc::new(remote::dispatch::TauriDispatcher::new(app_handle)),
            });
            remote::forward_pairing_notices(app.handle().clone(), &remote_server);
            let tray_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                remote_server.start_from_config().await;
                remote::tray::sync(&tray_handle).await;
                remote::refresh_funnel(&remote_server).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine::session_model_settings,
            engine::session_model_settings_save,
            engine::engine_chat,
            engine::steer::engine_chat_steer,
            engine::engine_plan_feedback,
            engine::engine_plan_approve,
            engine::engine_ask_response,
            engine::engine_ask_pending,
            engine::engine_autonomous_pause,
            engine::engine_autonomous_resume,
            engine::engine_autonomous_stop,
            engine::engine_cancel,
            engine::engine_compact,
            engine::engine_conversation_rewind,
            engine::engine_session_list,
            engine::engine_session_load,
            engine::engine_session_create,
            engine::engine_session_update_log,
            engine::engine_session_open,
            engine::engine_session_rename,
            engine::engine_session_delete,
            mentions::mention_search,
            rag_panel::rag_search,
            rag_panel::rag_article,
            dat_wiki::dat_wiki_schema,
            dat_wiki::dat_wiki_object,
            dat_wiki::dat_wiki_sheet,
            dat_wiki::dat_wiki_graphic,
            map_agent::map_agent_open,
            map_agent::map_agent_open_properties,
            map_agent::map_task_apply_undo,
            map_import::map_agent_import_open,
            map_import::map_import_bootstrap,
            map_import::map_import_source_pick,
            map_import::map_import_reference_list,
            map_import::map_import_reference_pick,
            map_import::map_import_source_render,
            map_import::map_import_source_objects,
            map_import::map_import_stamp_save,
            map_import::map_import_stamp_list,
            map_import::map_import_stamp_thumbnail,
            map_import::map_import_stamp_delete,
            map_agent::map_agent_bootstrap,
            map_agent::map_agent_session_list,
            map_agent::map_agent_session_create,
            map_agent::map_agent_session_load,
            map_agent::map_agent_session_rename,
            map_agent::map_agent_session_delete,
            map_agent::map_agent_conversation_reset,
            map_agent::map_agent_conversation_rewind,
            map_agent::map_agent_source_state,
            map_agent::map_agent_render,
            map_agent::map_agent_export_image,
            map_agent::map_agent_export_image_save,
            map_agent::map_agent_thumbnail,
            map_agent::map_agent_image_preview,
            map_agent::map_agent_image_confirm,
            map_agent::map_agent_stamp_preview,
            map_agent::map_agent_stamp_confirm,
            map_agent::map_agent_image_cancel,
            map_agent::map_agent_catalog,
            map_agent::map_agent_objects,
            map_agent::map_agent_diff_details,
            map_agent::map_agent_selection_save,
            map_agent::map_agent_selection_delete,
            map_agent::map_agent_candidate_state,
            map_agent::map_agent_candidate_revert,
            map_agent::map_agent_candidate_discard,
            map_agent::map_agent_candidate_apply,
            map_agent::map_agent_apply_undo,
            map_agent::map_agent_properties_save,
            map_agent::map_agent_chat,
            map_agent::map_agent_chat_steer,
            map_agent::map_agent_cancel,
            map_agent::map_agent_run_snapshot,
            attachment::stage_attachment,
            attachment::discard_attachment,
            ipc::app_settings,
            ipc::app_settings_save,
            remote::remote_status,
            remote::remote_settings_save,
            remote::remote_pair_begin,
            remote::remote_device_list,
            remote::remote_device_revoke,
            remote::remote_tailscale_serve,
            remote::remote_tailscale_install,
            remote::remote_tailscale_login,
            jev::jev_settings,
            jev::jev_settings_save,
            jev::jev_key_clear,
            ipc::notification_sound_preview,
            ipc::attention_notify,
            ipc::status,
            ipc::list,
            provider_service::provider_settings,
            provider_service::provider_status_list,
            provider_service::provider_install,
            provider_service::provider_login_start,
            provider_service::provider_login_status,
            provider_service::provider_login_cancel,
            provider_service::provider_api_key_save,
            provider_service::provider_base_url_save,
            provider_service::provider_logout,
            provider_service::provider_catalog,
            provider_service::provider_defaults_save,
            ipc::memory_get,
            ipc::memory_save,
            ipc::workspace_list,
            ipc::workspace_read,
            ipc::workspace_read_bytes,
            ipc::workspace_search,
            session_search::session_search,
            ipc::wiki_get,
            ipc::wiki_save,
            setup::setup_status,
            setup::euddraft_settings,
            setup::euddraft_check_update,
            setup::euddraft_update,
            project_launch::project_take_launch_request,
            project_launch::project_open_new_window,
            map_agent::map_agent_focus_main,
            setup::project_open,
            setup::project_recent_list,
            skills::skill_list,
            setup::project_recent_remove,
            setup::setup_pick_project_path,
            setup::setup_create_project,
            setup::setup_map_new_options,
            setup::setup_map_new_brushes,
            setup::setup_pick_starcraft_path,
            setup::setup_create_blank_project,
            setup::setup_pick_e3s_source,
            setup::setup_pick_import_destination,
            setup::setup_import_e3s,
            setup::setup_pick_e2_source,
            setup::setup_pick_e2_relink,
            setup::setup_e2_preflight,
            setup::setup_import_e2,
            setup::setup_cancel_e2_import,
            setup::project_export_e3s,
            setup::setup_pick_euddraft_path,
            setup::settings_pick_scmdraft_path,
            setup::project_open_scmdraft,
            setup::project_build_run,
            setup::project_open_root_in,
            setup::git_state,
            setup::git_consent_set,
            setup::git_log,
            setup::git_commit_detail,
            setup::git_revert,
            setup::setup_install_euddraft,
            setup::setup_provider_select,
            setup::bootstrap_run,
        ])
        .build(tauri::generate_context!())
        .expect("error while building eud-agent application")
        .run(|app, event| {
            // With remote access on, closing the last window leaves the
            // process (and its tray icon) running for paired devices.
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                let remote_enabled = app
                    .try_state::<std::sync::Arc<remote::RemoteServer>>()
                    .is_some_and(|server| server.is_enabled());
                if remote::tray::should_prevent_exit(code, remote_enabled) {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::ProgressEmitter;
    use parking_lot::Mutex;
    use std::path::PathBuf;

    #[derive(Default)]
    struct RecordingEmitter {
        events: Mutex<Vec<(String, u8, String)>>,
    }

    impl bootstrap::ProgressEmitter for RecordingEmitter {
        fn emit(&self, stage: &str, pct: u8, detail: &str) {
            self.events
                .lock()
                .push((stage.to_string(), pct, detail.to_string()));
        }
    }

    #[test]
    fn background_ffmpeg_ready_is_followed_by_terminal_done() {
        let emitter = RecordingEmitter::default();
        emitter.emit("bootstrap", 100, "audio converter ready");

        finish_background_ffmpeg_bootstrap(
            &emitter,
            Ok(bootstrap::ManagedFfmpegPaths {
                ffmpeg: PathBuf::from("ffmpeg.exe"),
                ffprobe: PathBuf::from("ffprobe.exe"),
                version: "test".to_string(),
            }),
        );

        assert_eq!(
            *emitter.events.lock(),
            vec![
                (
                    "bootstrap".to_string(),
                    100,
                    "audio converter ready".to_string(),
                ),
                ("bootstrap".to_string(), 100, "done".to_string()),
            ]
        );
    }

    #[test]
    fn background_ffmpeg_failure_is_non_blocking_and_terminal() {
        let emitter = RecordingEmitter::default();

        finish_background_ffmpeg_bootstrap(&emitter, Err(anyhow::anyhow!("download failed")));

        assert_eq!(
            *emitter.events.lock(),
            vec![
                (
                    "bootstrap".to_string(),
                    100,
                    "audio converter unavailable; chat and existing attachments remain available"
                        .to_string(),
                ),
                ("bootstrap".to_string(), 100, "done".to_string()),
            ]
        );
    }
}
