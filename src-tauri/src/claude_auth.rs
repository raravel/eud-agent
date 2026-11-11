//! Verified Claude Code native binary install and isolated subscription profile auth.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Config, DataDirs};

const RELEASE_BASE: &str = "https://downloads.claude.ai/claude-code-releases";
#[cfg(windows)]
const CLAUDE_BIN_FILENAME: &str = "claude.exe";
#[cfg(not(windows))]
const CLAUDE_BIN_FILENAME: &str = "claude";
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
    #[cfg(windows)]
    const HOME_VAR: &str = "USERPROFILE";
    #[cfg(not(windows))]
    const HOME_VAR: &str = "HOME";
    let user_home = std::env::var_os(HOME_VAR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    resolve_claude_cmd_with(dirs, config, user_home.as_deref(), || {
        which::which("claude").ok()
    })
}

/// Resolution order: explicit override, app-managed binary, `PATH`, then the official
/// native installer location. The last step exists because a GUI process launched from an
/// Explorer started before the installer updated the user `PATH` never inherits that entry.
fn resolve_claude_cmd_with(
    dirs: &DataDirs,
    config: &Config,
    user_home: Option<&Path>,
    path_lookup: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, String> {
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
    if let Some(path) = path_lookup() {
        return Ok(path);
    }
    user_home
        .map(|home| home.join(".local").join("bin").join(CLAUDE_BIN_FILENAME))
        .filter(|path| path.is_file())
        .ok_or_else(|| "provider_not_installed".to_string())
}

/// Import verification: surface the exact status cause instead of collapsing every
/// unauthenticated state into `provider_not_authenticated`.
pub fn require_authenticated(state: &ClaudeAuthState) -> Result<(), String> {
    if state.authenticated {
        return Ok(());
    }
    Err(state
        .detail_code
        .clone()
        .unwrap_or_else(|| "provider_not_authenticated".to_string()))
}

/// The profile's sign-in state. Read-only: it runs on every status refresh (every 2 s while a
/// sign-in is pending), so it never writes the profile or its ACLs, which would race the CLI
/// writing `.credentials.json`. Every decision is written to `provider-auth.log` when it
/// differs from the previous one, so a "로그인 필요" can be explained afterwards.
pub fn login_status(dirs: &DataDirs) -> ClaudeAuthState {
    let (state, reason) = probe_login_status(dirs);
    let summary = format!(
        "status authenticated={} compatible={} version={} code={} {reason}",
        state.authenticated,
        state.compatible,
        state.version.as_deref().unwrap_or("-"),
        state.detail_code.as_deref().unwrap_or("-"),
    );
    let mut last = LAST_STATUS.lock();
    if last.as_deref() != Some(summary.as_str()) {
        crate::provider_auth_log::record(dirs, "claude-code", &summary);
        *last = Some(summary);
    }
    state
}

static LAST_STATUS: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

fn probe_login_status(dirs: &DataDirs) -> (ClaudeAuthState, String) {
    let config = match dirs.load_config() {
        Ok(config) => config,
        Err(_) => {
            return (
                unavailable("provider_protocol_changed"),
                "reason=config-unreadable".to_string(),
            )
        }
    };
    let executable = match resolve_claude_cmd(dirs, &config) {
        Ok(executable) => executable,
        Err(code) => {
            return (
                unavailable(&code),
                "reason=executable-not-found".to_string(),
            )
        }
    };
    if !dirs.claude_config_dir().is_dir() {
        return (
            not_authenticated(None, true, "provider_not_authenticated"),
            "reason=profile-missing".to_string(),
        );
    }
    // A failed version probe is retried once: it says nothing about the sign-in, and
    // reporting it as one sends the user to a login that cannot help.
    let version = command_version(&executable, dirs).or_else(|_| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        command_version(&executable, dirs)
    });
    let (version, compatible) = match version {
        Ok(version) => {
            let compatible = version_supported(&version);
            (version, compatible)
        }
        Err(code) => {
            return (
                ClaudeAuthState {
                    resolved: true,
                    compatible: false,
                    authenticated: false,
                    version: None,
                    detail_code: Some(code.clone()),
                },
                format!("reason=version-probe-failed({code})"),
            )
        }
    };
    if !compatible {
        return (
            not_authenticated(Some(version), false, "provider_protocol_changed"),
            "reason=version-below-2.1.221".to_string(),
        );
    }
    let mut command = Command::new(executable);
    configure_command(&mut command, dirs);
    command
        .args(["auth", "status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return (
                unavailable("provider_transport_closed"),
                format!("reason=auth-status-not-started({})", error.kind()),
            )
        }
    };
    if !output.status.success() {
        return (
            not_authenticated(Some(version), true, "provider_not_authenticated"),
            format!(
                "reason=auth-status-exit({}) stderr={}",
                exit_label(output.status),
                first_line(&output.stderr)
            ),
        );
    }
    let status: ClaudeAuthStatus = match serde_json::from_slice(&output.stdout) {
        Ok(status) => status,
        Err(_) => {
            return (
                unavailable("provider_protocol_changed"),
                "reason=auth-status-unparsable".to_string(),
            )
        }
    };
    let subscription = status
        .subscription_type
        .as_deref()
        .or(status.auth_method.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let subscription_login = !subscription.contains("api") || subscription.contains("oauth");
    let credential = crate::claude_client::stored_credential_present(&dirs.claude_config_dir());
    let credential_label = match &credential {
        Ok(true) => "present".to_string(),
        Ok(false) => "missing".to_string(),
        Err(error) => format!("present-acl-failed({error})"),
    };
    let authenticated = status.logged_in && subscription_login && credential.unwrap_or(true);
    let reason = format!(
        "loggedIn={} authMethod={} subscription={} credential={credential_label}",
        status.logged_in,
        status.auth_method.as_deref().unwrap_or("-"),
        status.subscription_type.as_deref().unwrap_or("-"),
    );
    (
        ClaudeAuthState {
            resolved: true,
            compatible,
            authenticated,
            version: Some(version),
            detail_code: (!authenticated).then(|| "provider_not_authenticated".to_string()),
        },
        reason,
    )
}

fn not_authenticated(version: Option<String>, compatible: bool, code: &str) -> ClaudeAuthState {
    ClaudeAuthState {
        resolved: true,
        compatible,
        authenticated: false,
        version,
        detail_code: Some(code.to_string()),
    }
}

fn exit_label(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string())
}

fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("-")
        .to_string()
}

/// How long a `claude auth login` may wait for the browser before it is ended.
pub const LOGIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// A running `claude auth login`. Its output goes to `provider-auth.log` line by line; the
/// owner waits for it with `try_finish` and ends it with `kill` (a newer attempt, a cancel,
/// or `LOGIN_WAIT`), so no sign-in process outlives its attempt.
pub struct LoginProcess {
    child: std::process::Child,
    dirs: DataDirs,
}

pub fn login_start(dirs: &DataDirs) -> Result<LoginProcess, String> {
    let config = dirs
        .load_config()
        .map_err(|_| "provider_protocol_changed".to_string())?;
    let executable = resolve_claude_cmd(dirs, &config)?;
    ensure_profile(dirs)?;
    let mut command = Command::new(&executable);
    configure_command(&mut command, dirs);
    command
        .args(["auth", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let mut child = command.spawn().map_err(|error| {
        crate::provider_auth_log::record(
            dirs,
            "claude-code",
            &format!("login not started: {}", error.kind()),
        );
        "provider_transport_closed".to_string()
    })?;
    crate::provider_auth_log::record(
        dirs,
        "claude-code",
        &format!(
            "login started pid={} exe={}",
            child.id(),
            executable.display()
        ),
    );
    let pid = child.id();
    if let Some(stdout) = child.stdout.take() {
        forward_output(dirs.clone(), pid, "out", stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        forward_output(dirs.clone(), pid, "err", stderr);
    }
    Ok(LoginProcess {
        child,
        dirs: dirs.clone(),
    })
}

fn forward_output(
    dirs: DataDirs,
    pid: u32,
    stream: &'static str,
    reader: impl std::io::Read + Send + 'static,
) {
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(reader).lines().take(200) {
            let Ok(line) = line else { break };
            let line = line.trim();
            if !line.is_empty() {
                crate::provider_auth_log::record(
                    &dirs,
                    "claude-code",
                    &format!("login pid={pid} {stream}: {line}"),
                );
            }
        }
    });
}

impl LoginProcess {
    /// `None` while the process runs; once it exits, `Ok` for a successful sign-in (the
    /// stored credential is then made owner-only) or the code the attempt failed with.
    pub fn try_finish(&mut self) -> Option<Result<(), String>> {
        let status = match self.child.try_wait() {
            Ok(None) => return None,
            Ok(Some(status)) => status,
            Err(error) => {
                self.record(&format!("login wait failed: {}", error.kind()));
                return Some(Err("provider_transport_closed".to_string()));
            }
        };
        let credential =
            crate::claude_client::stored_credential_present(&self.dirs.claude_config_dir());
        self.record(&format!(
            "login exited pid={} exit={} credential={}",
            self.child.id(),
            exit_label(status),
            match &credential {
                Ok(true) => "present".to_string(),
                Ok(false) => "missing".to_string(),
                Err(error) => format!("present-acl-failed({error})"),
            }
        ));
        Some(if status.success() && !matches!(credential, Ok(false)) {
            Ok(())
        } else {
            Err("provider_login_failed".to_string())
        })
    }

    pub fn kill(&mut self, why: &str) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.record(&format!("login ended pid={} ({why})", self.child.id()));
        }
    }

    fn record(&self, event: &str) {
        crate::provider_auth_log::record(&self.dirs, "claude-code", event);
    }
}

impl Drop for LoginProcess {
    fn drop(&mut self) {
        self.kill("attempt dropped");
    }
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
    let checksum = manifest_checksum(&manifest_bytes, release_platform())?;
    let destination = dirs.claude_bin_dir().join(CLAUDE_BIN_FILENAME);
    fs::create_dir_all(dirs.claude_bin_dir())
        .map_err(|_| "provider install directory cannot be created".to_string())?;
    let temp = dirs
        .claude_bin_dir()
        .join(format!("claude-{version}-{}.tmp", uuid::Uuid::new_v4()));
    let response = client
        .get(format!(
            "{RELEASE_BASE}/{version}/{}/{CLAUDE_BIN_FILENAME}",
            release_platform()
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
        #[cfg(unix)]
        {
            drop(file);
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))
                .map_err(|_| "provider binary cannot be written".to_string())?;
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

/// Claude Code release-manifest platform key for this build target.
fn release_platform() -> &'static str {
    match (
        cfg!(windows),
        cfg!(target_os = "macos"),
        cfg!(target_arch = "aarch64"),
    ) {
        (true, _, true) => "win32-arm64",
        (true, _, false) => "win32-x64",
        (false, true, true) => "darwin-arm64",
        (false, true, false) => "darwin-x64",
        (false, false, true) => "linux-arm64",
        (false, false, false) => "linux-x64",
    }
}

fn ensure_profile(dirs: &DataDirs) -> Result<(), String> {
    fs::create_dir_all(dirs.claude_config_dir())
        .map_err(|_| "provider profile cannot be created".to_string())?;
    // Hardening the folder rewrites the ACL of every file in the profile, which races a CLI
    // writing its credential; do it only while the folder is not private yet.
    crate::provider_secrets::harden_private_path_if_needed(&dirs.claude_config_dir())?;
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
    if fs::read(&path).ok().as_deref() != Some(settings.as_slice()) {
        crate::memory::write_atomic_bytes(&path, &settings)
            .map_err(|_| "provider profile cannot be written".to_string())?;
    }
    crate::provider_secrets::harden_private_path_if_needed(&path)
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

/// Anthropic's Apple Developer ID team; the requirement pins an Apple-anchored
/// leaf certificate issued to that team.
#[cfg(target_os = "macos")]
const ANTHROPIC_CODE_REQUIREMENT: &str =
    "=anchor apple generic and certificate leaf[subject.OU] = \"Q6L2SF6YDW\"";

#[cfg(target_os = "macos")]
fn verify_anthropic_signature(path: &Path) -> Result<(), String> {
    let status = Command::new("/usr/bin/codesign")
        .arg("--verify")
        .arg("--strict")
        .arg(format!("-R{ANTHROPIC_CODE_REQUIREMENT}"))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "provider binary signature verification failed".to_string())?;
    if !status.success() {
        return Err("provider binary signature verification failed".to_string());
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn verify_anthropic_signature(_path: &Path) -> Result<(), String> {
    Err("provider binary signature verification is unavailable on this platform".to_string())
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
    fn resolution_falls_back_to_native_installer_location() {
        let base =
            std::env::temp_dir().join(format!("eud-claude-resolve-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let home = base.join("home");
        let native = home.join(".local").join("bin").join(CLAUDE_BIN_FILENAME);
        let config = Config::default();
        let no_path = || None;
        assert!(resolve_claude_cmd_with(&dirs, &config, None, no_path).is_err());
        assert!(resolve_claude_cmd_with(&dirs, &config, Some(&home), no_path).is_err());
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&native, b"").unwrap();
        assert_eq!(
            resolve_claude_cmd_with(&dirs, &config, Some(&home), no_path).unwrap(),
            native
        );
        // A PATH hit still takes precedence over the native installer copy.
        let on_path = base.join("path").join(CLAUDE_BIN_FILENAME);
        assert_eq!(
            resolve_claude_cmd_with(&dirs, &config, Some(&home), || Some(on_path.clone())).unwrap(),
            on_path
        );
        // A managed binary still wins over the native installer copy.
        let managed = dirs.claude_bin_dir().join(CLAUDE_BIN_FILENAME);
        fs::create_dir_all(dirs.claude_bin_dir()).unwrap();
        fs::write(&managed, b"").unwrap();
        assert_eq!(
            resolve_claude_cmd_with(&dirs, &config, Some(&home), no_path).unwrap(),
            managed
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn import_verification_reports_exact_status_cause() {
        assert_eq!(
            require_authenticated(&unavailable("provider_not_installed")),
            Err("provider_not_installed".to_string())
        );
        assert_eq!(
            require_authenticated(&ClaudeAuthState {
                resolved: true,
                compatible: true,
                authenticated: false,
                version: Some("2.1.274".to_string()),
                detail_code: None,
            }),
            Err("provider_not_authenticated".to_string())
        );
        assert_eq!(
            require_authenticated(&ClaudeAuthState {
                resolved: true,
                compatible: true,
                authenticated: true,
                version: Some("2.1.274".to_string()),
                detail_code: None,
            }),
            Ok(())
        );
    }

    fn shell(script: &str) -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("cmd");
            command.args(["/C", script]);
            command
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            command
        }
    }

    fn login_process(dirs: &DataDirs, script: &str) -> LoginProcess {
        let child = shell(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        LoginProcess {
            child,
            dirs: dirs.clone(),
        }
    }

    fn finish(process: &mut LoginProcess) -> Result<(), String> {
        for _ in 0..200 {
            if let Some(result) = process.try_finish() {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("login process did not exit");
    }

    #[test]
    fn login_exit_is_reported_and_logged() {
        let base = std::env::temp_dir().join(format!("eud-claude-login-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        fs::create_dir_all(dirs.claude_config_dir()).unwrap();

        // A failed exit is an error the panel shows, not a silent wait.
        let mut failed = login_process(&dirs, "exit 3");
        assert_eq!(
            finish(&mut failed),
            Err("provider_login_failed".to_string())
        );

        // A clean exit that stored no credential did not sign in either.
        let mut empty = login_process(&dirs, "exit 0");
        assert_eq!(finish(&mut empty), Err("provider_login_failed".to_string()));

        fs::write(dirs.claude_config_dir().join(".credentials.json"), b"{}").unwrap();
        let mut signed_in = login_process(&dirs, "exit 0");
        assert_eq!(finish(&mut signed_in), Ok(()));

        let log = fs::read_to_string(dirs.logs_dir().join("provider-auth.log")).unwrap();
        assert!(log.contains("exit=3 credential=missing"));
        assert!(log.contains("exit=0 credential=present"));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_replaced_or_dropped_login_process_is_ended() {
        let base = std::env::temp_dir().join(format!("eud-claude-login-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let long = if cfg!(windows) {
            "ping -n 60 127.0.0.1 >NUL"
        } else {
            "sleep 60"
        };
        let mut replaced = login_process(&dirs, long);
        replaced.kill("replaced");
        assert!(matches!(replaced.child.try_wait(), Ok(Some(_))));

        let dropped = login_process(&dirs, long);
        let pid = dropped.child.id();
        drop(dropped);
        let log = fs::read_to_string(dirs.logs_dir().join("provider-auth.log")).unwrap();
        assert!(log.contains(&format!("login ended pid={pid} (attempt dropped)")));
        fs::remove_dir_all(base).ok();
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
