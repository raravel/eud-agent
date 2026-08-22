//! Branded Windows review notifications with click-through session routing.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager};
use tauri_winrt_notification::{IconCrop, Toast};
use windows_registry::CURRENT_USER;

const NOTIFICATION_ICON_RESOURCE: &str = "icons/notification.png";
static IDENTITY_REGISTRATION: OnceLock<Result<(), String>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationActivatedEvent {
    pub session_id: String,
}

fn notification_icon(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve(NOTIFICATION_ICON_RESOURCE, BaseDirectory::Resource)
        .map_err(|error| format!("failed to resolve notification icon: {error}"))
}

fn register_identity(app: &AppHandle, icon: &Path) -> Result<(), String> {
    let app_id = &app.config().identifier;
    let display_name = app.config().product_name.as_deref().unwrap_or("eud-agent");
    let key = CURRENT_USER
        .create(format!(r"SOFTWARE\Classes\AppUserModelId\{app_id}"))
        .map_err(|error| format!("failed to register notification identity: {error}"))?;
    key.set_string("DisplayName", display_name)
        .map_err(|error| format!("failed to register notification display name: {error}"))?;
    key.set_string("IconBackgroundColor", "0")
        .map_err(|error| format!("failed to register notification icon background: {error}"))?;
    key.set_hstring("IconUri", &icon.into())
        .map_err(|error| format!("failed to register notification icon: {error}"))?;
    Ok(())
}

fn ensure_identity(app: &AppHandle, icon: &Path) -> Result<(), String> {
    IDENTITY_REGISTRATION
        .get_or_init(|| register_identity(app, icon))
        .clone()
}

pub fn show(app: &AppHandle, title: &str, body: &str, session_id: &str) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("notification session id is empty".to_string());
    }

    let icon = notification_icon(app)?;
    ensure_identity(app, &icon)?;

    let activation_app = app.clone();
    let activation_session_id = session_id.to_string();
    Toast::new(&app.config().identifier)
        .title(title)
        .text1(body)
        .icon(&icon, IconCrop::Square, "eud-agent")
        .sound(None)
        .on_activated(move |_| {
            if let Some(window) = activation_app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            if let Err(error) = activation_app.emit(
                "notification_activated",
                NotificationActivatedEvent {
                    session_id: activation_session_id.clone(),
                },
            ) {
                eprintln!("eud-agent: notification activation emit failed: {error}");
            }
            Ok(())
        })
        .show()
        .map_err(|error| format!("failed to show notification: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_payload_uses_camel_case_session_id() {
        assert_eq!(
            serde_json::to_value(NotificationActivatedEvent {
                session_id: "session-a".to_string(),
            })
            .unwrap(),
            serde_json::json!({"sessionId": "session-a"})
        );
    }
}
