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
pub mod tools;

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
            if let Ok(bridge) = ipc::bridge_from_config(&data_dirs) {
                bridge.cleanup_stale();
            }
            app.manage(ipc::BridgeManaged::new(data_dirs));

            let app_handle = app.handle().clone();
            let sink = engine::TauriEventSink::new(app_handle.clone());
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let driver = engine::ProductionCodexDriver::new(cwd, sink.clone());
            let config =
                engine::AgentEngineConfig::new("[project state]\n(unavailable)", None, Vec::new());

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running eud-agent application");
}
