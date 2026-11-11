//! App-profile Claude subscription credential: read-only access for the model catalog.
//!
//! The Claude CLI owns its OAuth pair. Refresh tokens are single-use and rotate on every
//! refresh, so a second refresher racing the CLI (or losing a rotated pair it failed to write
//! back) makes the server reject the stored refresh token, after which the CLI clears the pair
//! and every turn fails until the user logs in again. The app therefore never refreshes or
//! writes the credential: an expired access token only degrades the catalog to the CLI-managed
//! default, and the CLI refreshes the pair itself on its next turn. Tokens are never logged.
//!
//! On macOS the CLI keeps the pair in the login Keychain instead of the file (see
//! `keychain`); the file remains the fallback it uses when the Keychain is unavailable.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use zeroize::Zeroizing;

const CREDENTIALS_FILENAME: &str = ".credentials.json";
const OAUTH_OBJECT_KEY: &str = "claudeAiOauth";
/// A token this close to expiry is treated as expired so a paginated catalog fetch never
/// straddles the boundary.
const EXPIRY_SKEW: Duration = Duration::from_secs(60);
const MAX_CREDENTIAL_BYTES: u64 = 1024 * 1024;

/// The stored access token for one catalog request, as-is. An expired or missing one is an
/// error; the app never refreshes it (see the module docs).
pub(crate) fn access_token(profile_dir: &Path) -> Result<Zeroizing<String>, String> {
    let document = read_document(profile_dir)?;
    let deadline = now_ms().saturating_add(EXPIRY_SKEW.as_millis() as u64);
    access_token_valid_at(oauth_object(&document)?, deadline)
}

fn credential_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(CREDENTIALS_FILENAME)
}

/// Whether the profile holds a stored subscription credential, without reading the secret
/// itself (a Keychain attribute query never raises an access prompt). A present file is made
/// owner-only when it is not already; `Err` means the file is present but that failed, which
/// is a reason to report, never a reason to call the profile signed out.
pub(crate) fn stored_credential_present(profile_dir: &Path) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    if keychain::present(profile_dir) {
        return Ok(true);
    }
    let path = credential_path(profile_dir);
    if !path.is_file() {
        return Ok(false);
    }
    crate::provider_secrets::harden_private_path_if_needed(&path).map(|()| true)
}

fn read_document(profile_dir: &Path) -> Result<Value, String> {
    #[cfg(target_os = "macos")]
    if let Some(bytes) = keychain::read(profile_dir)? {
        return serde_json::from_slice(&bytes)
            .map_err(|_| "provider_not_authenticated".to_string());
    }
    let path = credential_path(profile_dir);
    let metadata =
        std::fs::metadata(&path).map_err(|_| "provider_not_authenticated".to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CREDENTIAL_BYTES {
        return Err("provider_not_authenticated".to_string());
    }
    let bytes =
        Zeroizing::new(std::fs::read(&path).map_err(|_| "provider_not_authenticated".to_string())?);
    serde_json::from_slice(&bytes).map_err(|_| "provider_not_authenticated".to_string())
}

/// The CLI's macOS credential store: one login-Keychain generic password per config
/// directory, service `Claude Code-credentials-<first 8 hex of sha256(CLAUDE_CONFIG_DIR)>`,
/// account the login user name.
#[cfg(target_os = "macos")]
mod keychain {
    use std::path::Path;

    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    use security_framework::passwords;
    use sha2::{Digest, Sha256};
    use zeroize::Zeroizing;

    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    fn service(profile_dir: &Path) -> String {
        let digest = format!(
            "{:x}",
            Sha256::digest(profile_dir.to_string_lossy().as_bytes())
        );
        format!("Claude Code-credentials-{}", &digest[..8])
    }

    /// The stored item's account, read from attributes only.
    fn account(service: &str) -> Option<String> {
        let results = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .load_attributes(true)
            .limit(Limit::Max(1))
            .search()
            .ok()?;
        results
            .first()?
            .simplify_dict()?
            .remove("acct")
            .filter(|account| !account.is_empty())
    }

    pub(super) fn present(profile_dir: &Path) -> bool {
        account(&service(profile_dir)).is_some()
    }

    pub(super) fn read(profile_dir: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        let service = service(profile_dir);
        let Some(account) = account(&service) else {
            return Ok(None);
        };
        match passwords::get_generic_password(&service, &account) {
            Ok(bytes) if bytes.len() as u64 <= super::MAX_CREDENTIAL_BYTES => {
                Ok(Some(Zeroizing::new(bytes)))
            }
            Ok(_) => Err("provider_not_authenticated".to_string()),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(_) => Err("provider_not_authenticated".to_string()),
        }
    }
}

fn oauth_object(document: &Value) -> Result<&Map<String, Value>, String> {
    document
        .get(OAUTH_OBJECT_KEY)
        .and_then(Value::as_object)
        .ok_or_else(|| "provider_not_authenticated".to_string())
}

fn access_token_valid_at(
    oauth: &Map<String, Value>,
    at_ms: u64,
) -> Result<Zeroizing<String>, String> {
    if let Some(expires_at) = oauth.get("expiresAt").and_then(Value::as_u64) {
        if expires_at <= at_ms {
            return Err("provider_auth_expired".to_string());
        }
    }
    token_field(oauth, "accessToken").ok_or_else(|| "provider_not_authenticated".to_string())
}

fn token_field(oauth: &Map<String, Value>, key: &str) -> Option<Zeroizing<String>> {
    oauth
        .get(key)
        .and_then(Value::as_str)
        .filter(|token| is_token_shaped(token))
        .map(|token| Zeroizing::new(token.to_string()))
}

fn is_token_shaped(token: &str) -> bool {
    !token.is_empty() && !token.chars().any(char::is_control)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FAR_FUTURE_MS: u64 = 32_503_680_000_000;

    fn profile() -> PathBuf {
        let base = std::env::temp_dir().join(format!("eud-claude-token-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_credential(profile: &Path, oauth: Value) {
        std::fs::write(
            credential_path(profile),
            serde_json::to_vec(&json!({ OAUTH_OBJECT_KEY: oauth })).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn access_token_requires_unexpired_oauth_credential() {
        let base = profile();
        assert_eq!(
            access_token(&base),
            Err("provider_not_authenticated".to_string())
        );
        write_credential(&base, json!({"accessToken": "live", "expiresAt": 1}));
        assert_eq!(
            access_token(&base),
            Err("provider_auth_expired".to_string())
        );
        write_credential(
            &base,
            json!({"accessToken": "live", "expiresAt": FAR_FUTURE_MS}),
        );
        assert_eq!(access_token(&base).unwrap().as_str(), "live");
        std::fs::write(credential_path(&base), br#"{"primaryApiKey":"sk"}"#).unwrap();
        assert_eq!(
            access_token(&base),
            Err("provider_not_authenticated".to_string())
        );
        std::fs::remove_dir_all(base).ok();
    }

    /// The CLI is the only refresher: an expired pair stays byte-for-byte as the CLI wrote it,
    /// with no lock directory left behind.
    #[test]
    fn expired_credential_is_never_refreshed_or_rewritten() {
        let base = profile();
        write_credential(
            &base,
            json!({
                "accessToken": "stale-access",
                "refreshToken": "refresh-1",
                "expiresAt": 1,
                "refreshTokenExpiresAt": FAR_FUTURE_MS
            }),
        );
        let before = std::fs::read(credential_path(&base)).unwrap();
        assert_eq!(
            access_token(&base),
            Err("provider_auth_expired".to_string())
        );
        assert_eq!(std::fs::read(credential_path(&base)).unwrap(), before);
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 1);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_token_inside_the_expiry_skew_counts_as_expired() {
        let base = profile();
        write_credential(
            &base,
            json!({"accessToken": "live", "expiresAt": now_ms() + 10_000}),
        );
        assert_eq!(
            access_token(&base),
            Err("provider_auth_expired".to_string())
        );
        std::fs::remove_dir_all(base).ok();
    }
}
