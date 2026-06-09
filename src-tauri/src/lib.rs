//! eud-agent Tauri 2 application shell.
//!
//! This is the scaffold entry point for the v2 standalone desktop app. It wires the
//! `tauri::Builder`, registers the shell and dialog plugins, and opens the main window
//! that hosts the prebuilt React panel (`../panel/dist`). The typed Tauri IPC surface is
//! registered here; engine, tools, codex_client, isom, bridge_io, and memory are wired by
//! later tasks.

pub mod bootstrap;
pub mod bridge_io;
pub mod codex_client;
pub mod config;
pub mod engine;
pub mod ipc;
pub mod mapsafe;
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
        .invoke_handler(tauri::generate_handler![
            ipc::chat,
            ipc::plan_feedback,
            ipc::plan_approve,
            ipc::changeset_decision,
            ipc::cancel,
            ipc::reset,
            ipc::status,
            ipc::list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running eud-agent application");
}
