//! eud-agent Tauri 2 application shell.
//!
//! This is the scaffold entry point for the v2 standalone desktop app. It wires the
//! `tauri::Builder`, registers the shell and dialog plugins, and opens the main window
//! that hosts the prebuilt React panel (`../panel/dist`). The typed Tauri IPC surface is
//! registered here; engine, tools, codex_client, isom, bridge_io, and memory are wired by
//! later tasks.

use tauri::Manager;

pub mod bootstrap;
pub mod bridge_io;
pub mod chk;
pub mod codex_client;
pub mod config;
pub mod engine;
pub mod ipc;
pub mod journal;
pub mod mapsafe;
pub mod memory;
pub mod rag;
pub mod setup;
pub mod tools;

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

    fn append_episode(&self, episode: &serde_json::Value) -> bool {
        self.current_memory().append_episode(episode)
    }
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
        .setup(|app| {
            let data_dirs = config::DataDirs::resolve(&*app)?;
            // Non-fatal: a failed dir create resurfaces on first write with context.
            if let Err(error) = data_dirs.ensure_dirs() {
                eprintln!("eud-agent: cannot create data dirs: {error}");
            }
            if let Ok(bridge) = ipc::bridge_from_config(&data_dirs) {
                bridge.cleanup_stale();
            }
            app.manage(ipc::BridgeManaged::new(data_dirs.clone()));

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
                }
            });

            let app_handle = app.handle().clone();
            let sink = engine::TauriEventSink::new(app_handle.clone());
            // Stable app-owned cwd for codex (rules.md), NOT the launch dir:
            // `tauri dev` runs from the repo, so current_dir made codex pick up
            // the repo's AGENTS.md (hivemind instructions) and treat the Rust
            // repo as its workspace instead of the EUD map project.
            let cwd = data_dirs.codex_workspace_dir();
            let driver = engine::ProductionCodexDriver::new(cwd, sink.clone());
            let config =
                engine::AgentEngineConfig::new("[project state]\n(unavailable)", None, Vec::new())
                    .with_memory_provider(std::sync::Arc::new(AppMemoryProvider {
                        dirs: data_dirs,
                    }));

            app.manage(tokio::sync::Mutex::new(engine::AgentEngine::new(
                driver, sink, config,
            )));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine::engine_chat,
            engine::engine_plan_feedback,
            engine::engine_plan_approve,
            engine::engine_changeset_decision,
            engine::engine_cancel,
            engine::engine_reset,
            ipc::status,
            ipc::list,
            ipc::memory_get,
            ipc::memory_save,
            setup::setup_status,
            setup::setup_pick_editor_path,
            setup::bootstrap_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running eud-agent application");
}
