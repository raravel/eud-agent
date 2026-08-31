//! Verified Claude Code native binary install and isolated subscription profile auth.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Config, DataDirs};

const RELEASE_BASE: &str = "https://downloads.claude.ai/claude-code-releases";
const CLAUDE_BIN_FILENAME: &str = "claude.exe";
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 350 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeAuthState {
    pub resolved: bool,
    pub compatible: bool,
    pub authenticated: bool,
    pub version: Option<String>,
    pub detail_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeAuthStatus {
    logged_in: bool,
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(default)]
    auth_method: Option<String>,
}

pub fn resolve_claude_cmd(dirs: &DataDirs, config: &Config) -> Result<PathBuf, String> {
    if let Some(path) = config
        .providers
        .claude_code
        .executable_override
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| "provider_not_installed".to_string());
    }
    let managed = dirs.claude_bin_dir().join(CLAUDE_BIN_FILENAME);
    if managed.is_file() {
        return Ok(managed);
    }
    which::which("claude").map_err(|_| "provider_not_installed".to_string())
}

pub fn login_status(dirs: &DataDirs) -> ClaudeAuthState {
    let config = match dirs.load_config() {
        Ok(config) => config,
        Err(_) => return unavailable("provider_protocol_changed"),
    };
    let executable = match resolve_claude_cmd(dirs, &config) {
        Ok(executable) => executable,
        Err(code) => return unavailable(&code),
    };
    if ensure_profile(dirs).is_err() {
        return unavailable("provider_protocol_changed");
    }
    let version = command_version(&executable, dirs).ok();
    let compatible = version.as_deref().is_some_and(version_supported);
    let mut command = Command::new(executable);
    configure_command(&mut command, dirs);
    command
        .args(["auth", "status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let output = match command.output() {
        Ok(output) => output,
        Err(_) => return unavailable("provider_transport_closed"),
    };
    if !output.status.success() {
        return ClaudeAuthState {
            resolved: true,
            compatible,
            authenticated: false,
            version,
            detail_code: Some("provider_not_authenticated".to_string()),
        };
    }
    let status: ClaudeAuthStatus = match serde_json::from_slice(&output.stdout) {
        Ok(status) => status,
        Err(_) => return unavailable("provider_protocol_changed"),
    };
    let subscription = status
        .subscription_type
        .as_deref()
        .or(status.auth_method.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let subscription_login = !subscription.contains("api") || subscription.contains("oauth");
    let credential = dirs.claude_config_dir().join(".credentials.json");
    let authenticated = compatible
        && status.logged_in
        && subscription_login
        && credential.is_file()
        && crate::provider_secrets::harden_private_path(&credential).is_ok();
    ClaudeAuthState {
        resolved: true,
        compatible,
        authenticated,
        version,
        detail_code: (!authenticated).then(|| "provider_not_authenticated".to_string()),
    }
}

pub fn login_start(dirs: &DataDirs) -> Result<(), String> {
    let config = dirs
        .load_config()
        .map_err(|_| "provider_protocol_changed".to_string())?;
    let executable = resolve_claude_cmd(dirs, &config)?;
    ensure_profile(dirs)?;
    let mut command = Command::new(executable);
    configure_command(&mut command, dirs);
    command
        .args(["auth", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| "provider_transport_closed".to_string())
}

pub fn logout(dirs: &DataDirs) -> Result<(), String> {
    let config = dirs
        .load_config()
        .map_err(|_| "provider_protocol_changed".to_string())?;
    let executable = resolve_claude_cmd(dirs, &config)?;
    let mut command = Command::new(executable);
    configure_command(&mut command, dirs);
    command
        .args(["auth", "logout"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let status = command
        .status()
        .map_err(|_| "provider_transport_closed".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("provider_not_authenticated".to_string())
    }
}

pub async fn install(dirs: &DataDirs) -> Result<ClaudeAuthState, String> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "provider_transport_closed".to_string())?;
    let version = bounded_text(
        client
            .get(format!("{RELEASE_BASE}/latest"))
            .send()
            .await
            .map_err(|_| "provider_transport_closed".to_string())?,
        128,
    )
    .await?;
    let version = version.trim();
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err("provider_protocol_changed".to_string());
    }
    let manifest_bytes = bounded_bytes(
        client
            .get(format!("{RELEASE_BASE}/{version}/manifest.json"))
            .send()
            .await
            .map_err(|_| "provider_transport_closed".to_string())?,
        MAX_MANIFEST_BYTES as u64,
    )
    .await?;
    let checksum = manifest_checksum(&manifest_bytes, windows_platform())?;
    let destination = dirs.claude_bin_dir().join(CLAUDE_BIN_FILENAME);
    fs::create_dir_all(dirs.claude_bin_dir())
        .map_err(|_| "provider install directory cannot be created".to_string())?;
    let temp = dirs
        .claude_bin_dir()
        .join(format!("claude-{version}-{}.tmp", uuid::Uuid::new_v4()));
    let response = client
        .get(format!(
            "{RELEASE_BASE}/{version}/{}/claude.exe",
            windows_platform()
        ))
        .send()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_BINARY_BYTES)
    {
        return Err("provider_transport_closed".to_string());
    }
    let result = async {
        let mut file = fs::File::create(&temp)
            .map_err(|_| "provider binary temp file cannot be created".to_string())?;
        let mut response = response;
        let mut size = 0_u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "provider_transport_closed".to_string())?
        {
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| "provider binary is too large".to_string())?;
            if size > MAX_BINARY_BYTES {
                return Err("provider binary is too large".to_string());
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .map_err(|_| "provider binary cannot be written".to_string())?;
        }
        file.sync_all()
            .map_err(|_| "provider binary cannot be written".to_string())?;
        let actual = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual != checksum {
            return Err("provider binary checksum mismatch".to_string());
        }
        verify_anthropic_signature(&temp)?;
        if destination.exists() {
            fs::remove_file(&destination)
                .map_err(|_| "provider binary cannot be replaced".to_string())?;
        }
        fs::rename(&temp, &destination)
            .map_err(|_| "provider binary cannot be published".to_string())?;
        ensure_profile(dirs)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    Ok(login_status(dirs))
}

fn manifest_checksum(bytes: &[u8], platform: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "provider_protocol_changed".to_string())?;
    let checksum = value
        .pointer(&format!("/platforms/{platform}/checksum"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "provider_protocol_changed".to_string())?;
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(checksum.to_ascii_lowercase())
}

fn windows_platform() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "win32-arm64"
    } else {
        "win32-x64"
    }
}

fn ensure_profile(dirs: &DataDirs) -> Result<(), String> {
    fs::create_dir_all(dirs.claude_config_dir())
        .map_err(|_| "provider profile cannot be created".to_string())?;
    crate::provider_secrets::harden_private_path(&dirs.claude_config_dir())?;
    let settings = serde_json::to_vec_pretty(&serde_json::json!({
        "env": {
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_UPDATES": "1"
        },
        "permissions": {
            "defaultMode": "dontAsk"
        }
    }))
    .map_err(|_| "provider profile cannot be serialized".to_string())?;
    let path = dirs.claude_config_dir().join("settings.json");
    crate::memory::write_atomic_bytes(&path, &settings)
        .map_err(|_| "provider profile cannot be written".to_string())?;
    crate::provider_secrets::harden_private_path(&path)
}

pub(crate) fn configure_command(command: &mut Command, dirs: &DataDirs) {
    command.env("CLAUDE_CONFIG_DIR", dirs.claude_config_dir());
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_PROFILE",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_ORGANIZATION_ID",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_SIMPLE",
    ] {
        command.env_remove(name);
    }
}

fn command_version(executable: &Path, dirs: &DataDirs) -> Result<String, String> {
    let mut command = Command::new(executable);
    configure_command(&mut command, dirs);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let output = command
        .output()
        .map_err(|_| "provider_transport_closed".to_string())?;
    if !output.status.success() {
        return Err("provider_protocol_changed".to_string());
    }
    let version =
        String::from_utf8(output.stdout).map_err(|_| "provider_protocol_changed".to_string())?;
    Ok(version.trim().chars().take(64).collect())
}

fn version_supported(version: &str) -> bool {
    let mut parts = version
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let parsed = [
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    ];
    parsed >= [2, 1, 221]
}

fn unavailable(code: &str) -> ClaudeAuthState {
    ClaudeAuthState {
        resolved: code != "provider_not_installed",
        compatible: false,
        authenticated: false,
        version: None,
        detail_code: Some(code.to_string()),
    }
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

async fn bounded_text(response: reqwest::Response, max: u64) -> Result<String, String> {
    let bytes = bounded_bytes(response, max).await?;
    String::from_utf8(bytes).map_err(|_| "provider_protocol_changed".to_string())
}

async fn bounded_bytes(response: reqwest::Response, max: u64) -> Result<Vec<u8>, String> {
    if !response.status().is_success()
        || response.content_length().is_some_and(|length| length > max)
    {
        return Err("provider_transport_closed".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "provider_transport_closed".to_string())?;
    if bytes.len() as u64 > max {
        return Err("provider_protocol_changed".to_string());
    }
    Ok(bytes.to_vec())
}

#[cfg(windows)]
fn verify_anthropic_signature(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
        WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOCATION_CHECK_CHAIN, WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        pPolicyCallbackData: std::ptr::null_mut(),
        pSIPClientData: std::ptr::null_mut(),
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        hWVTStateData: std::ptr::null_mut(),
        pwszURLReference: std::ptr::null_mut(),
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN,
        dwUIContext: 0,
        pSignatureSettings: std::ptr::null_mut(),
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    let signer_name = if status == 0 {
        unsafe {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if provider.is_null() {
                std::ptr::null_mut()
            } else {
                WTHelperGetProvSignerFromChain(provider, 0, 0, 0)
            };
            let certificate = if signer.is_null() {
                std::ptr::null_mut()
            } else {
                WTHelperGetProvCertFromChain(signer, 0)
            };
            if certificate.is_null() || (*certificate).pCert.is_null() {
                None
            } else {
                let cert = (*certificate).pCert;
                let length = CertGetNameStringW(
                    cert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                );
                if length <= 1 {
                    None
                } else {
                    let mut name = vec![0_u16; length as usize];
                    CertGetNameStringW(
                        cert,
                        CERT_NAME_SIMPLE_DISPLAY_TYPE,
                        0,
                        std::ptr::null(),
                        name.as_mut_ptr(),
                        length,
                    );
                    Some(String::from_utf16_lossy(&name[..name.len() - 1]))
                }
            }
        }
    } else {
        None
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
    }
    if status != 0 || signer_name.as_deref() != Some("Anthropic, PBC") {
        return Err("provider binary signature verification failed".to_string());
    }
    Ok(())
}

#[cfg(not(windows))]
fn verify_anthropic_signature(_path: &Path) -> Result<(), String> {
    Err("provider binary signature verification requires Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_requires_exact_platform_checksum() {
        let manifest = br#"{"platforms":{"win32-x64":{"checksum":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}"#;
        assert_eq!(
            manifest_checksum(manifest, "win32-x64").unwrap(),
            "a".repeat(64)
        );
        assert!(manifest_checksum(manifest, "win32-arm64").is_err());
        assert!(manifest_checksum(
            br#"{"platforms":{"win32-x64":{"checksum":"bad"}}}"#,
            "win32-x64"
        )
        .is_err());
    }

    #[test]
    fn command_environment_removes_ambient_api_credentials() {
        let base = std::env::temp_dir().join(format!("eud-claude-env-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let mut command = Command::new("claude");
        command.env("ANTHROPIC_API_KEY", "must-not-survive");
        configure_command(&mut command, &dirs);
        let config_dir = command
            .get_envs()
            .find(|(name, _)| *name == "CLAUDE_CONFIG_DIR")
            .and_then(|(_, value)| value);
        assert_eq!(config_dir, Some(dirs.claude_config_dir().as_os_str()));
        assert!(command
            .get_envs()
            .any(|(name, value)| name == "ANTHROPIC_API_KEY" && value.is_none()));
        fs::remove_dir_all(base).ok();
    }
}
