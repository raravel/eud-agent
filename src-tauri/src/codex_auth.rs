//! codex CLI login state + the guided first-run login flow.
//!
//! codex must be both RESOLVABLE (on PATH / `CODEX_CMD`) and AUTHENTICATED before
//! the agent can run a turn — an unauthenticated codex fails every turn with an
//! auth error. This module probes auth via `codex login status` (exit 0 = logged
//! in) and drives the two login paths the setup screen offers: ChatGPT OAuth
//! (`codex login`, opens a browser) and an API key (`codex login --with-api-key`,
//! read from stdin — NEVER argv).
//!
//! Everything here is synchronous (`std::process`) so it composes into the
//! existing `setup_status` `spawn_blocking` probe; the Tauri commands wrap each
//! call in `spawn_blocking` to keep the IPC thread free.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::codex_client::resolve_codex_cmd;

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

/// Probe codex auth with `codex login status`. Exit 0 means logged in; a resolve
/// failure reports `resolved: false` so the setup screen can guide installation.
pub fn login_status() -> CodexAuthState {
    let codex = match resolve_codex_cmd() {
        Ok(path) => path,
        Err(error) => return CodexAuthState::unresolved(error.to_string()),
    };

    match Command::new(&codex)
        .args(["login", "status"])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => {
            let authed = output.status.success();
            let stream = if authed {
                &output.stdout
            } else {
                &output.stderr
            };
            let detail = first_line(&String::from_utf8_lossy(stream)).unwrap_or_else(|| {
                if authed {
                    "logged in".to_string()
                } else {
                    "not logged in".to_string()
                }
            });
            CodexAuthState {
                resolved: true,
                authed,
                detail,
            }
        }
        Err(error) => CodexAuthState {
            resolved: true,
            authed: false,
            detail: format!("could not run codex login status: {error}"),
        },
    }
}

/// Launch the interactive ChatGPT OAuth login (`codex login`), which opens a
/// browser. Spawned DETACHED — codex runs its own loopback callback server and
/// writes the auth on success, so this returns as soon as it is launched and the
/// panel polls [`login_status`] until it flips. The child is intentionally not
/// awaited and not killed on drop.
pub fn login_oauth() -> Result<(), String> {
    let codex = resolve_codex_cmd().map_err(|error| error.to_string())?;
    Command::new(&codex)
        .arg("login")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_child| ())
        .map_err(|error| format!("failed to launch codex login: {error}"))
}

/// Log in with an API key piped to `codex login --with-api-key` over stdin (never
/// argv). Awaits completion and re-probes status, so the returned state reflects
/// the post-login session.
pub fn login_api_key(api_key: &str) -> Result<CodexAuthState, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("API key is empty".to_string());
    }
    let codex = resolve_codex_cmd().map_err(|error| error.to_string())?;

    let mut child = Command::new(&codex)
        .args(["login", "--with-api-key"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to spawn codex login: {error}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "codex login stdin was not piped".to_string())?;
        stdin
            .write_all(key.as_bytes())
            .map_err(|error| format!("failed to write API key to codex login: {error}"))?;
        // Drop closes stdin (EOF) so `--with-api-key` stops reading.
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("codex login did not complete: {error}"))?;

    if output.status.success() {
        Ok(login_status())
    } else {
        Err(first_line(&String::from_utf8_lossy(&output.stderr))
            .unwrap_or_else(|| "codex login with API key failed".to_string()))
    }
}

/// First non-empty trimmed line of a command's output (the user-facing summary).
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Progress sink for the codex download: the setup screen shows a spinner
/// (`codexBusy`) rather than a progress bar, so install progress is dropped.
struct NoopEmitter;

impl crate::bootstrap::ProgressEmitter for NoopEmitter {
    fn emit(&self, _stage: &str, _pct: u8, _detail: &str) {}
}

/// Download + install the standalone codex binary, then report the refreshed
/// login state. After placement `resolve_codex_cmd` finds it (well-known path),
/// so the returned state has `resolved: true` (and `authed: false` until the
/// user logs in).
#[tauri::command]
pub async fn codex_install(
    state: tauri::State<'_, crate::ipc::BridgeManaged>,
) -> Result<CodexAuthState, String> {
    let dirs = state.dirs().clone();
    crate::bootstrap::ensure_codex(&dirs, &NoopEmitter)
        .await
        .map_err(|error| format!("{error:#}"))?;
    tauri::async_runtime::spawn_blocking(login_status)
        .await
        .map_err(|error| error.to_string())
}

/// Report codex login state (resolved + authenticated) for the setup gate.
#[tauri::command]
pub async fn codex_login_status() -> Result<CodexAuthState, String> {
    tauri::async_runtime::spawn_blocking(login_status)
        .await
        .map_err(|error| error.to_string())
}

/// Launch the ChatGPT OAuth login; the panel then polls `codex_login_status`.
#[tauri::command]
pub async fn codex_login_start() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(login_oauth)
        .await
        .map_err(|error| error.to_string())?
}

/// Log in with an API key (read from stdin) and return the refreshed state.
/// The JS arg is `key` (single word) to avoid camelCase/snake_case ambiguity.
#[tauri::command]
pub async fn codex_login_with_api_key(key: String) -> Result<CodexAuthState, String> {
    tauri::async_runtime::spawn_blocking(move || login_api_key(&key))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_skips_blank_leading_lines() {
        assert_eq!(
            first_line("\n  \nLogged in using ChatGPT\nextra"),
            Some("Logged in using ChatGPT".to_string())
        );
        assert_eq!(first_line("   "), None);
        assert_eq!(first_line(""), None);
    }

    #[test]
    fn unresolved_state_is_not_authed() {
        let state = CodexAuthState::unresolved("codex not found".to_string());
        assert!(!state.resolved);
        assert!(!state.authed);
        assert_eq!(state.detail, "codex not found");
    }

    #[test]
    fn empty_api_key_is_rejected_before_spawning() {
        // Guards the stdin-only contract: a blank key never reaches codex.
        assert_eq!(login_api_key("   "), Err("API key is empty".to_string()));
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
