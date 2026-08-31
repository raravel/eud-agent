//! Codex CLI distribution login state + the guided first-run install/login flow.
//!
//! Codex must be RESOLVABLE with both runtime-helper siblings and AUTHENTICATED before the
//! agent can run a turn — an incomplete distribution fails Code Mode or elevated-sandbox
//! setup, while an unauthenticated CLI fails every turn with an auth error. This module probes
//! `codex login status` and drives the setup screen's install, ChatGPT OAuth (`codex login`),
//! and API-key (`codex login --with-api-key`, key read from stdin — NEVER argv) paths.
//!
//! Everything here is synchronous (`std::process`) so it composes into the
//! existing `setup_status` `spawn_blocking` probe; the Tauri commands wrap each
//! call in `spawn_blocking` to keep the IPC thread free.

use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::codex_client::{ensure_codex_profile, resolve_codex_cmd_for};

/// Suppress the console window when spawning the `codex.cmd` batch shim from the
/// windowless GUI app. Without `CREATE_NO_WINDOW` (0x0800_0000) Windows opens a
/// terminal for each codex invocation; no-op on other platforms.
#[allow(unused_variables)]
fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
}

/// codex login state surfaced to the setup screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexAuthState {
    /// codex CLI was found (PATH / `CODEX_CMD`).
    pub resolved: bool,
    /// `codex login status` reported a logged-in session (exit 0).
    pub authed: bool,
    /// One-line human-readable status / error (never a raw identifier).
    pub detail: String,
}

impl CodexAuthState {
    fn unresolved(detail: String) -> Self {
        Self {
            resolved: false,
            authed: false,
            detail,
        }
    }
}

fn command_for(dirs: &crate::config::DataDirs) -> Result<Command, String> {
    ensure_codex_profile(dirs)?;
    let config = dirs
        .load_config()
        .map_err(|_| "provider_protocol_changed".to_string())?;
    let executable =
        resolve_codex_cmd_for(dirs, &config).map_err(|_| "provider_not_installed".to_string())?;
    let mut command = Command::new(executable);
    command.env("CODEX_HOME", dirs.codex_home_dir());
    Ok(command)
}

pub fn login_status(dirs: &crate::config::DataDirs) -> CodexAuthState {
    let mut command = match command_for(dirs) {
        Ok(command) => command,
        Err(detail) => return CodexAuthState::unresolved(detail),
    };
    command.args(["login", "status"]).stdin(Stdio::null());
    hide_console(&mut command);
    match command.output() {
        Ok(output) => {
            let credential = dirs.codex_home_dir().join("auth.json");
            let authed = output.status.success()
                && credential.is_file()
                && crate::provider_secrets::harden_private_path(&credential).is_ok();
            CodexAuthState {
                resolved: true,
                authed,
                detail: if authed {
                    "ready".to_string()
                } else {
                    "provider_not_authenticated".to_string()
                },
            }
        }
        Err(_) => CodexAuthState {
            resolved: true,
            authed: false,
            detail: "provider_transport_closed".to_string(),
        },
    }
}

pub fn login_oauth(dirs: &crate::config::DataDirs) -> Result<(), String> {
    let mut command = command_for(dirs)?;
    command
        .arg("login")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| "provider_transport_closed".to_string())
}

pub fn login_api_key(
    dirs: &crate::config::DataDirs,
    api_key: &str,
) -> Result<CodexAuthState, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("provider_credential_missing".to_string());
    }
    let mut command = command_for(dirs)?;
    command
        .args(["login", "--with-api-key"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| "provider_transport_closed".to_string())?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "provider_transport_closed".to_string())?;
        stdin
            .write_all(key.as_bytes())
            .map_err(|_| "provider_transport_closed".to_string())?;
    }
    let status = child
        .wait()
        .map_err(|_| "provider_transport_closed".to_string())?;
    if status.success() {
        Ok(login_status(dirs))
    } else {
        Err("provider_not_authenticated".to_string())
    }
}

pub fn logout(dirs: &crate::config::DataDirs) -> Result<(), String> {
    let mut command = command_for(dirs)?;
    command
        .arg("logout")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_state_is_not_authed() {
        let state = CodexAuthState::unresolved("codex not found".to_string());
        assert!(!state.resolved);
        assert!(!state.authed);
        assert_eq!(state.detail, "codex not found");
    }

    #[test]
    fn empty_api_key_is_rejected_before_spawning() {
        let base = std::env::temp_dir().join(format!("eud-codex-auth-{}", uuid::Uuid::new_v4()));
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        assert_eq!(
            login_api_key(&dirs, "   "),
            Err("provider_credential_missing".to_string())
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn auth_state_round_trips_as_json() {
        let state = CodexAuthState {
            resolved: true,
            authed: false,
            detail: "not logged in".to_string(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: CodexAuthState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }
}
