//! Remote access from a phone over Tailscale.
//!
//! The app serves `/remote` on `127.0.0.1:<port>` only; the user publishes it
//! inside their tailnet with `tailscale serve`. Every API call needs a device
//! token issued through a one-time pairing code shown in Settings.

pub mod auth;
pub mod caller;
pub mod dispatch;
pub mod hub;
pub mod server;
pub mod settings;
pub mod tailscale;

use std::sync::Arc;

use serde::Serialize;

pub use auth::DeviceView;
pub use server::RemoteServer;
pub use settings::{RemoteDevice, RemoteSettings};

/// What the settings screen shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub port: u16,
    pub listening: bool,
    pub error: Option<String>,
    pub tailscale: tailscale::TailscaleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStart {
    pub code: String,
    pub expires_at: u64,
    /// `https://<dnsName>/remote/#pair=<code>` when the MagicDNS name is known.
    pub url: Option<String>,
}

pub fn pairing_url(dns_name: Option<&str>, code: &str) -> Option<String> {
    dns_name.map(|name| format!("https://{name}/remote/#pair={code}"))
}

async fn status_payload(server: &RemoteServer) -> Result<RemoteStatus, String> {
    let remote = server
        .dirs()
        .load_config()
        .map_err(|error| error.to_string())?
        .remote;
    let listener = server.listener_state().await;
    Ok(RemoteStatus {
        enabled: remote.enabled,
        port: remote.port,
        listening: listener.listening,
        error: listener.error,
        tailscale: tailscale::status(remote.port).await,
    })
}

/// Remote server, Tailscale, and listener state for Settings → 원격 접속.
#[tauri::command]
pub async fn remote_status(
    server: tauri::State<'_, Arc<RemoteServer>>,
) -> Result<RemoteStatus, String> {
    status_payload(&server).await
}

/// Save on/off and port, then start, restart, or stop the server to match.
#[tauri::command]
pub async fn remote_settings_save(
    server: tauri::State<'_, Arc<RemoteServer>>,
    enabled: bool,
    port: u16,
) -> Result<RemoteStatus, String> {
    settings::validate_port(port)?;
    {
        let _config_guard = crate::config::PROJECT_CONFIG_LOCK.lock();
        let dirs = server.dirs();
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        config.remote.enabled = enabled;
        config.remote.port = port;
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
    }
    server.apply(enabled, port).await;
    status_payload(&server).await
}

/// Issue a one-time pairing code (replacing any live one).
#[tauri::command]
pub async fn remote_pair_begin(
    server: tauri::State<'_, Arc<RemoteServer>>,
) -> Result<PairingStart, String> {
    let remote = server
        .dirs()
        .load_config()
        .map_err(|error| error.to_string())?
        .remote;
    if !remote.enabled {
        return Err("원격 접속을 먼저 켜 주세요.".to_string());
    }
    let code = server.auth().begin_pairing()?;
    let tailscale = tailscale::status(remote.port).await;
    Ok(PairingStart {
        url: pairing_url(tailscale.dns_name.as_deref(), &code.code),
        code: code.code,
        expires_at: code.expires_at,
    })
}

#[tauri::command]
pub async fn remote_device_list(
    server: tauri::State<'_, Arc<RemoteServer>>,
) -> Result<Vec<DeviceView>, String> {
    Ok(server.auth().devices())
}

/// Disconnect one device; returns the remaining devices.
#[tauri::command]
pub async fn remote_device_revoke(
    server: tauri::State<'_, Arc<RemoteServer>>,
    id: String,
) -> Result<Vec<DeviceView>, String> {
    let auth = server.auth().clone();
    tauri::async_runtime::spawn_blocking(move || {
        auth.revoke(&id)?;
        Ok(auth.devices())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Run `tailscale serve` for the saved port, then report the new status.
#[tauri::command]
pub async fn remote_tailscale_serve(
    server: tauri::State<'_, Arc<RemoteServer>>,
) -> Result<RemoteStatus, String> {
    let port = server
        .dirs()
        .load_config()
        .map_err(|error| error.to_string())?
        .remote
        .port;
    tailscale::serve(port).await?;
    status_payload(&server).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_url_needs_the_magic_dns_name() {
        assert_eq!(
            pairing_url(Some("pc.tail1234.ts.net"), "ABCD2345").as_deref(),
            Some("https://pc.tail1234.ts.net/remote/#pair=ABCD2345")
        );
        assert_eq!(pairing_url(None, "ABCD2345"), None);
    }
}
