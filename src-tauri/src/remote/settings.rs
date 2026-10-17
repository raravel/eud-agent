//! Persisted remote-access preferences (`Config.remote`).
//!
//! Off by default. Device tokens are never stored: each paired device keeps
//! only the SHA-256 of its token.

use serde::{Deserialize, Serialize};

/// Default loopback port the remote server listens on.
pub const DEFAULT_REMOTE_PORT: u16 = 47821;
/// Ports below this are reserved for system services.
pub const MIN_REMOTE_PORT: u16 = 1024;

const fn default_port() -> u16 {
    DEFAULT_REMOTE_PORT
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub devices: Vec<RemoteDevice>,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_REMOTE_PORT,
            devices: Vec::new(),
        }
    }
}

/// One paired device. `token_sha256` is the lowercase hex SHA-256 of its token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
    pub token_sha256: String,
    /// Unix milliseconds.
    pub created_at: u64,
    /// Unix milliseconds of the last authenticated request (persisted at most once a minute).
    #[serde(default)]
    pub last_seen_at: Option<u64>,
    /// The tailnet login (`Tailscale-User-Login`) that paired the device
    /// through `tailscale serve`; `None` for a device paired on the PC itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_login: Option<String>,
}

/// Reject ports the server cannot or should not bind.
pub fn validate_port(port: u16) -> Result<(), String> {
    if port < MIN_REMOTE_PORT {
        return Err(format!(
            "포트는 {MIN_REMOTE_PORT}부터 65535 사이여야 합니다. 다른 포트를 입력해 주세요."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn older_config_without_remote_loads_with_remote_off() {
        let config: Config =
            serde_json::from_str(r#"{"schema_version":3,"deep_planning":true}"#).unwrap();
        assert_eq!(config.remote, RemoteSettings::default());
        assert!(!config.remote.enabled);
        assert_eq!(config.remote.port, DEFAULT_REMOTE_PORT);
        assert!(config.deep_planning);
    }

    #[test]
    fn remote_settings_round_trip_in_camel_case() {
        let config = Config {
            remote: RemoteSettings {
                enabled: true,
                port: 50000,
                devices: vec![RemoteDevice {
                    id: "d1".to_string(),
                    name: "폰".to_string(),
                    token_sha256: "ab".repeat(32),
                    created_at: 1,
                    last_seen_at: Some(2),
                    owner_login: Some("me@example.com".to_string()),
                }],
            },
            ..Config::default()
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["remote"]["devices"][0]["tokenSha256"], "ab".repeat(32));
        assert_eq!(json["remote"]["devices"][0]["lastSeenAt"], 2);
        assert_eq!(json["remote"]["devices"][0]["ownerLogin"], "me@example.com");
        let back: Config = serde_json::from_value(json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn partial_remote_object_fills_defaults() {
        let config: Config =
            serde_json::from_str(r#"{"schema_version":3,"remote":{"enabled":true}}"#).unwrap();
        assert!(config.remote.enabled);
        assert_eq!(config.remote.port, DEFAULT_REMOTE_PORT);
        assert!(config.remote.devices.is_empty());
    }

    #[test]
    fn privileged_ports_are_rejected() {
        assert!(validate_port(80).is_err());
        assert!(validate_port(0).is_err());
        assert!(validate_port(MIN_REMOTE_PORT).is_ok());
        assert!(validate_port(65535).is_ok());
    }
}
