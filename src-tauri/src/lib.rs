//! eud-agent Tauri 2 application shell.
//!
//! This is the scaffold entry point for the v2 standalone desktop app. It wires the
//! `tauri::Builder`, registers the shell and dialog plugins, and opens the main window
//! that hosts the prebuilt React panel (`../panel/dist`). The typed Tauri IPC surface is
//! registered here; engine, tools, codex_client, isom, bridge_io, and memory are wired by
//! later tasks.

use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager};

pub mod antigravity_auth;
pub mod antigravity_client;
pub mod attachment;
pub mod audio;
pub mod bootstrap;
pub mod bridge_install;
pub mod bridge_io;
pub mod chk;
pub mod claude_auth;
pub mod claude_client;
pub mod codex_auth;
pub mod codex_client;
pub mod config;
pub mod context_state;
pub mod edd_runner;
pub mod engine;
pub mod eps_preflight;
pub mod harness;
pub mod ipc;
pub mod journal;
pub mod map_agent;
pub mod map_candidate;
pub mod map_context;
pub mod map_image;
pub mod map_import;
pub mod map_model;
pub mod map_stamp;
pub mod map_verify;
pub mod mapsafe;
pub mod mcp;
pub mod memory;
pub mod mentions;
pub mod ollama;
pub mod opencode_go;
pub mod provider;
pub mod provider_secrets;
pub mod provider_service;
pub mod provider_tool_loop;
pub mod provider_transcript;
pub mod rag;
pub mod session;
pub mod setup;
pub mod task_state;
pub mod tool_exec;
pub mod tools;
pub mod trace_test;
pub mod wiki;
#[cfg(windows)]
pub mod windows_notification;
pub mod workspace;
pub mod write_coordinator;

#[derive(Clone)]
struct AppMemoryProvider {
    dirs: config::DataDirs,
}

impl AppMemoryProvider {
    fn current_memory(&self) -> memory::ProjectMemory {
        match ipc::bridge_from_config(&self.dirs).and_then(|bridge| {
            bridge
                .read_status_snapshot(bridge_io::HEARTBEAT_STALE_AFTER)
                .map_err(|error| error.to_string())
        }) {
            Ok(snapshot) => memory::ProjectMemory::new(self.dirs.memory_dir(), snapshot.project),
            Err(_) => memory::ProjectMemory::new(self.dirs.memory_dir(), ""),
        }
    }
}

impl engine::MemoryProvider for AppMemoryProvider {
    fn render_section(&self) -> String {
        self.current_memory().render_section(None)
    }
}

/// Resolves the current project from the editor STATUS and reads/writes its
/// dat-edit wiki ledger, so `[wiki facts]` and the accept-hook always target the
/// project the editor currently has open (like [`AppMemoryProvider`]).
#[derive(Clone)]
struct AppWikiProvider {
    dirs: config::DataDirs,
}

impl AppWikiProvider {
    fn current_project(&self) -> String {
        match ipc::bridge_from_config(&self.dirs).and_then(|bridge| {
            bridge
                .read_status_snapshot(bridge_io::HEARTBEAT_STALE_AFTER)
                .map_err(|error| error.to_string())
        }) {
            Ok(snapshot) => snapshot.project,
            Err(_) => String::new(),
        }
    }

    fn current_store(&self) -> wiki::WikiStore {
        wiki::WikiStore::load(self.dirs.wiki_dir(&self.current_project()))
    }
}

impl engine::WikiProvider for AppWikiProvider {
    fn render_section(&self, query: &str) -> Option<String> {
        self.current_store().render_section(query)
    }

    fn record_accepted(&self, entries: Vec<wiki::LedgerEntry>) -> Option<ipc::WikiResponse> {
        let mut store = self.current_store();
        if !store.enabled() || entries.is_empty() {
            return None;
        }
        for entry in entries {
            store.upsert(entry);
        }
        if let Err(error) = store.save() {
            eprintln!("eud-agent: wiki ledger save failed: {error}");
            return None;
        }
        Some(ipc::WikiResponse::from(store.ledger()))
    }
}

/// Renders `[project state]` fresh each turn from the editor's `status.txt`
/// (project name + build state), resolving the bridge from `config.json` so an
/// editor-path change or a downed editor takes effect without a restart.
#[derive(Clone)]
struct AppProjectStateProvider {
    dirs: config::DataDirs,
}

impl engine::ProjectStateProvider for AppProjectStateProvider {
    fn render_section(&self) -> String {
        let bridge = match ipc::bridge_from_config(&self.dirs) {
            Ok(bridge) => bridge,
            Err(error) => return format!("[project state]\n(editor not connected: {error})"),
        };
        match bridge.read_status_snapshot(bridge_io::HEARTBEAT_STALE_AFTER) {
            Ok(snapshot) => {
                let project = if snapshot.project.trim().is_empty() {
                    "(no project open)"
                } else {
                    snapshot.project.trim()
                };
                format!(
                    "[project state]\nproject={project}\ncompiling={}",
                    snapshot.compiling
                )
            }
            Err(error) => format!("[project state]\n(editor not connected: {error})"),
        }
    }
}

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

/// Build and run the Tauri application.
///
/// Kept out of `main.rs` so the same setup is reusable by mobile targets and
/// integration tests (idiomatic Tauri 2 lib/bin split).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            isom::assert_abi_version()?;
            let data_dirs = config::DataDirs::resolve(&*app)?;
            // Non-fatal: a failed dir create resurfaces on first write with context.
            if let Err(error) = data_dirs.ensure_dirs() {
                eprintln!("eud-agent: cannot create data dirs: {error}");
            }
            let removed_episodes = memory::cleanup_legacy_episode_files(&data_dirs.memory_dir());
            if removed_episodes > 0 {
                eprintln!(
                    "eud-agent: removed {removed_episodes} obsolete project-memory episode files"
                );
            }
            let attachment_store = attachment::AttachmentStore::new(data_dirs.attachments_dir());
            attachment_store.cleanup_stale_drafts();
            if let Ok(bridge) = ipc::bridge_from_config(&data_dirs) {
                bridge.cleanup_stale();
            }

            // Keep the editor's installed Lua bridge in sync with the copy bundled in
            // this app: a self-update may ship a newer bridge, re-installed here on the
            // next launch (no manual `install_bridge.ps1` re-run). Best-effort — a downed
            // or moved editor must never block startup.
            if let Ok(config) = data_dirs.load_config() {
                let editor_path = config.editor_path.trim();
                if !editor_path.is_empty()
                    && config::validate_editor_path(std::path::Path::new(editor_path))
                {
                    match app
                        .path()
                        .resolve("bridge/ZZZ_10_agent_bridge.lua", BaseDirectory::Resource)
                    {
                        Ok(bundled) => {
                            match bridge_install::sync_bridge(
                                &bundled,
                                std::path::Path::new(editor_path),
                            ) {
                                Ok(true) => eprintln!("eud-agent: bridge re-installed to editor"),
                                Ok(false) => {}
                                Err(error) => {
                                    eprintln!("eud-agent: bridge sync skipped: {error}")
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!("eud-agent: cannot resolve bundled bridge resource: {error}")
                        }
                    }
                }
            }
            let provider_service = provider_service::ProviderService::new(data_dirs.clone())
                .map_err(anyhow::Error::msg)?;
            app.manage(provider_service.clone());

            app.manage(ipc::BridgeManaged::new(data_dirs.clone()));
            app.manage(attachment::AttachmentManaged::new(attachment_store.clone()));

            // Feature 10 boot flow (EUD-132): on later launches where the editor
            // path is already configured but an asset went missing/corrupt,
            // re-download in the background. The very first run is panel-driven
            // (setup screen -> pick folder -> bootstrap_run), and readiness is
            // never gated on this task — failures surface to the panel as
            // `progress {stage: bootstrap, detail: "error: ..."}` with retry.
            let boot_handle = app.handle().clone();
            let boot_dirs = data_dirs.clone();
            tauri::async_runtime::spawn(async move {
                let check_dirs = boot_dirs.clone();
                // The manifest check hashes the RAG index; keep it off the runtime.
                let auto = tauri::async_runtime::spawn_blocking(move || {
                    setup::should_auto_bootstrap(&check_dirs)
                })
                .await
                .unwrap_or(false);
                if auto {
                    // run_bootstrap already emitted the failure to the panel.
                    let _ = setup::run_bootstrap(&boot_handle, &boot_dirs).await;
                } else {
                    let emitter = bootstrap::TauriEmitter(boot_handle.clone());
                    if !bootstrap::managed_ffmpeg_ready(&boot_dirs) {
                        let result = bootstrap::ensure_ffmpeg(&boot_dirs, &emitter).await;
                        finish_background_ffmpeg_bootstrap(&emitter, result);
                    }
                }
            });

            let app_handle = app.handle().clone();

            // Resolve and checksum-verify the pinned adapter before constructing
            // the shared runtime. Failure is represented by an unavailable
            // analyzer and never blocks application startup or other tools.
            let adapter = app.path().resolve(
                "vendor/epscript-lsp-agent/adapter.cjs",
                BaseDirectory::Resource,
            );
            let checksum = app.path().resolve(
                "vendor/epscript-lsp-agent/adapter.sha256",
                BaseDirectory::Resource,
            );
            let analyzer = match (adapter, checksum) {
                (Ok(adapter), Ok(checksum)) => eps_preflight::NodeEpsAnalyzer::from_resource(
                    adapter,
                    checksum,
                    data_dirs.logs_dir(),
                    data_dirs.lsp_workspaces_dir(),
                ),
                (Err(error), _) | (_, Err(error)) => eps_preflight::NodeEpsAnalyzer::unavailable(
                    eps_preflight::SkipReason::AdapterMissing,
                    format!("cannot resolve bundled epscript adapter: {error}"),
                ),
            };

            let activity_handle = app_handle.clone();
            let writes = write_coordinator::ProjectWriteCoordinator::new(move |event| {
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
            let imports = map_import::MapImportStore::new(data_dirs.clone());
            if let Err(error) = imports.cleanup_startup() {
                eprintln!("eud-agent: map import cleanup skipped: {error}");
            }
            app.manage(imports.clone());
            let candidates = map_candidate::CandidateStore::new(data_dirs.clone(), imports);
            if let Err(error) = candidates.cleanup_startup() {
                eprintln!("eud-agent: candidate cache cleanup skipped: {error}");
            }
            app.manage(map_agent::MapAgentService::new(
                data_dirs.clone(),
                candidates.clone(),
                writes.clone(),
            ));
            let services = tool_exec::ToolServices::new(
                data_dirs.clone(),
                std::sync::Arc::new(analyzer),
                candidates,
                writes,
            );
            let mentions = services.mentions();
            app.manage(mentions.clone());
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(error) = mentions.warmup() {
                    eprintln!("eud-agent: mention context warmup skipped: {error}");
                }
            });

            // Warm the RAG embedding model in the background; readiness NEVER
            // gates startup (search_docs returns zero hits until it is ready,
            // which still lifts the evidence gate).
            let warm_rag = services.rag();
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

            let config =
                engine::AgentEngineConfig::new("[project state]\n(unavailable)", None, Vec::new())
                    .with_memory_provider(std::sync::Arc::new(AppMemoryProvider {
                        dirs: data_dirs.clone(),
                    }))
                    .with_wiki_provider(std::sync::Arc::new(AppWikiProvider {
                        dirs: data_dirs.clone(),
                    }))
                    .with_project_state_provider(std::sync::Arc::new(AppProjectStateProvider {
                        dirs: data_dirs.clone(),
                    }));

            app.manage(engine::SessionEngineManager::new(
                session_store,
                attachment_store,
                services,
                provider_service,
                config,
                app_handle,
                data_dirs.clone(),
                data_dirs.codex_workspace_dir(),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine::session_model_settings,
            engine::session_model_settings_save,
            engine::engine_chat,
            engine::engine_plan_feedback,
            engine::engine_plan_approve,
            engine::engine_changeset_decision,
            engine::engine_harness_jobs,
            engine::engine_harness_runtime_confirm,
            engine::engine_harness_skip,
            engine::engine_harness_dismiss,
            engine::engine_harness_retry,
            engine::engine_harness_decision,
            engine::engine_ask_response,
            engine::engine_ask_pending,
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
            map_agent::map_agent_open,
            map_import::map_agent_import_open,
            map_import::map_import_bootstrap,
            map_import::map_import_source_pick,
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
            map_agent::map_agent_source_state,
            map_agent::map_agent_render,
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
            map_agent::map_agent_candidate_revert,
            map_agent::map_agent_candidate_discard,
            map_agent::map_agent_candidate_apply,
            map_agent::map_agent_apply_undo,
            map_agent::map_agent_chat,
            map_agent::map_agent_cancel,
            attachment::stage_attachment,
            attachment::discard_attachment,
            ipc::app_settings,
            ipc::app_settings_save,
            ipc::notification_sound_preview,
            ipc::attention_notify,
            ipc::status,
            ipc::launch_editor,
            ipc::list,
            provider_service::provider_settings,
            provider_service::provider_status_list,
            provider_service::provider_install,
            provider_service::provider_login_start,
            provider_service::provider_login_status,
            provider_service::provider_login_cancel,
            provider_service::provider_credential_import,
            provider_service::provider_api_key_save,
            provider_service::provider_base_url_save,
            provider_service::provider_logout,
            provider_service::provider_catalog,
            provider_service::provider_defaults_save,
            ipc::memory_get,
            ipc::memory_save,
            ipc::workspace_list,
            ipc::workspace_read,
            ipc::workspace_search,
            ipc::wiki_get,
            ipc::wiki_save,
            setup::setup_status,
            setup::setup_pick_editor_path,
            setup::setup_provider_select,
            setup::bootstrap_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running eud-agent application");
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
