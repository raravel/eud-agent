//! Provider credential storage and explicit ambient CLI credential import rails.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::config::DataDirs;
use crate::provider::ProviderId;

const IMPORT_MAX_BYTES: u64 = 1024 * 1024;
const SECRET_TARGET_PREFIX: &str = "eud-agent/providers";

#[derive(Debug, Clone)]
pub struct ProviderSecretStore {
    dirs: DataDirs,
    user_home: PathBuf,
}

impl ProviderSecretStore {
    pub fn new(dirs: DataDirs) -> Result<Self, String> {
        let user_home = std::env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "provider credential profile is unavailable".to_string())?;
        Ok(Self { dirs, user_home })
    }

    #[cfg(test)]
    pub(crate) fn with_home(dirs: DataDirs, user_home: PathBuf) -> Self {
        Self { dirs, user_home }
    }

    pub fn save_secret(
        &self,
        provider: ProviderId,
        name: &str,
        secret: &str,
    ) -> Result<(), String> {
        validate_secret_name(name)?;
        if secret.is_empty() {
            return Err("provider credential is empty".to_string());
        }
        write_os_secret(&secret_target(provider, name), secret.as_bytes())
    }

    pub fn read_secret(&self, provider: ProviderId, name: &str) -> Result<Option<String>, String> {
        validate_secret_name(name)?;
        let Some(bytes) = read_os_secret(&secret_target(provider, name))? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "provider credential store returned invalid UTF-8".to_string())
    }

    pub fn delete_secret(&self, provider: ProviderId, name: &str) -> Result<(), String> {
        validate_secret_name(name)?;
        delete_os_secret(&secret_target(provider, name))
    }

    pub fn ambient_source(&self, provider: ProviderId) -> Result<PathBuf, String> {
        match provider {
            ProviderId::Codex => Ok(self.user_home.join(".codex").join("auth.json")),
            ProviderId::ClaudeCode => Ok(self.user_home.join(".claude").join(".credentials.json")),
            _ => Err("provider_import_unavailable".to_string()),
        }
    }

    pub fn cli_credential_destination(&self, provider: ProviderId) -> Result<PathBuf, String> {
        match provider {
            ProviderId::Codex => Ok(self.dirs.codex_home_dir().join("auth.json")),
            ProviderId::ClaudeCode => Ok(self.dirs.claude_config_dir().join(".credentials.json")),
            _ => Err("provider_import_unavailable".to_string()),
        }
    }

    /// Copy only the fixed credential payload. The verifier runs against the app
    /// profile; failure restores the previous app credential and never touches the source.
    pub fn import_cli_credential(
        &self,
        provider: ProviderId,
        verify: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let source = self.ambient_source(provider)?;
        let destination = self.cli_credential_destination(provider)?;
        let source_before = read_checked_credential(&source, provider)?;
        let source_hash = Sha256::digest(&source_before);
        let previous = fs::read(&destination).ok();
        write_private_atomic(&destination, &source_before)?;

        if let Err(error) = verify() {
            match previous {
                Some(previous) => write_private_atomic(&destination, &previous)?,
                None => match fs::remove_file(&destination) {
                    Ok(()) => {}
                    Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("provider import rollback failed".to_string()),
                },
            }
            return Err(redact_error(&error));
        }

        let source_after = fs::read(&source)
            .map_err(|_| "provider credential source changed during import".to_string())?;
        if Sha256::digest(&source_after) != source_hash {
            return Err("provider credential source changed during import".to_string());
        }
        Ok(())
    }

    pub fn logout_cli_profile(&self, provider: ProviderId) -> Result<(), String> {
        let destination = self.cli_credential_destination(provider)?;
        match fs::remove_file(destination) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("provider logout failed".to_string()),
        }
    }
}

fn validate_secret_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid provider credential name".to_string());
    }
    Ok(())
}

fn secret_target(provider: ProviderId, name: &str) -> String {
    format!("{SECRET_TARGET_PREFIX}/{provider}/{name}")
}

fn read_checked_credential(path: &Path, provider: ProviderId) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "provider_import_unavailable".to_string())?;
    if !metadata.file_type().is_file() || is_reparse_point(&metadata) {
        return Err("provider credential source is not a regular file".to_string());
    }
    if metadata.len() == 0 || metadata.len() > IMPORT_MAX_BYTES {
        return Err("provider credential source size is invalid".to_string());
    }
    let bytes =
        fs::read(path).map_err(|_| "provider credential source cannot be read".to_string())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "provider credential source is invalid".to_string())?;
    let object = value
        .as_object()
        .filter(|object| !object.is_empty())
        .ok_or_else(|| "provider credential source is invalid".to_string())?;
    let recognized = match provider {
        ProviderId::Codex => ["tokens", "OPENAI_API_KEY", "auth_mode"]
            .iter()
            .any(|key| object.contains_key(*key)),
        ProviderId::ClaudeCode => ["claudeAiOauth", "oauthAccount", "primaryApiKey"]
            .iter()
            .any(|key| object.contains_key(*key)),
        _ => false,
    };
    if !recognized {
        return Err("provider credential source has an unsupported shape".to_string());
    }
    Ok(bytes)
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "provider credential destination is invalid".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|_| "provider credential directory cannot be created".to_string())?;
    reject_reparse_ancestors(parent)?;
    harden_private_path(parent)?;
    let temp = parent.join(format!(".credential-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| "provider credential temp file cannot be created".to_string())?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "provider credential temp file cannot be written".to_string())?;
        ensure_private_file(&temp)?;
        if path.exists() {
            fs::remove_file(path)
                .map_err(|_| "provider credential destination cannot be replaced".to_string())?;
        }
        fs::rename(&temp, path)
            .map_err(|_| "provider credential destination cannot be published".to_string())?;
        ensure_private_file(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn reject_reparse_ancestors(path: &Path) -> Result<(), String> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            let metadata = fs::symlink_metadata(candidate)
                .map_err(|_| "provider credential directory cannot be inspected".to_string())?;
            if is_reparse_point(&metadata) {
                return Err("provider credential directory contains a reparse point".to_string());
            }
        }
        current = candidate.parent();
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub fn harden_private_path(path: &Path) -> Result<(), String> {
    harden_private_path_impl(path)
}

#[cfg(windows)]
fn harden_private_path_impl(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_SUCCESS, GENERIC_ALL, HANDLE, HLOCAL,
    };
    use windows_sys::Win32::Security::Authorization::{
        BuildTrusteeWithSidW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
        GRANT_ACCESS, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let metadata = fs::metadata(path)
        .map_err(|_| "provider credential ACL cannot be inspected".to_string())?;
    if metadata.file_attributes() & 0x400 != 0 {
        return Err("provider credential path is a reparse point".to_string());
    }

    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("provider credential ACL cannot read the current user".to_string());
    }
    let result = (|| {
        let mut required = 0_u32;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required) };
        if required < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err("provider credential ACL returned an invalid user SID".to_string());
        }
        let mut token_user = vec![0_u8; required as usize];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_user.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err("provider credential ACL cannot read the current user".to_string());
        }
        let sid: PSID = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut access: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
        access.grfAccessPermissions = GENERIC_ALL;
        access.grfAccessMode = GRANT_ACCESS;
        access.grfInheritance = if metadata.is_dir() {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        };
        unsafe { BuildTrusteeWithSidW(&mut access.Trustee, sid) };
        let mut acl = std::ptr::null_mut();
        if unsafe { SetEntriesInAclW(1, &access, std::ptr::null(), &mut acl) } != ERROR_SUCCESS {
            return Err("provider credential ACL cannot be created".to_string());
        }
        let mut wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        };
        unsafe { LocalFree(acl as HLOCAL) };
        if status != ERROR_SUCCESS {
            return Err("provider credential ACL cannot be applied".to_string());
        }
        Ok(())
    })();
    unsafe { CloseHandle(token) };
    result
}

#[cfg(not(windows))]
fn harden_private_path_impl(_path: &Path) -> Result<(), String> {
    Err("provider credential profiles require Windows ACLs".to_string())
}

fn ensure_private_file(path: &Path) -> Result<(), String> {
    harden_private_path(path)
}

pub fn redact_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("token")
        || lower.contains("api key")
        || lower.contains("authorization")
        || lower.contains("credential")
        || lower.contains("email")
    {
        "provider credential operation failed".to_string()
    } else {
        message.chars().take(256).collect()
    }
}

#[cfg(windows)]
fn write_os_secret(target: &str, secret: &[u8]) -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    let mut target = target.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut user = "eud-agent"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut blob = secret.to_vec();
    let credential = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        Comment: std::ptr::null_mut(),
        LastWritten: windows_sys::Win32::Foundation::FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        },
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: user.as_mut_ptr(),
    };
    let ok = unsafe { CredWriteW(&credential, 0) };
    blob.zeroize();
    target.zeroize();
    user.zeroize();
    if ok == 0 {
        return Err("provider credential store write failed".to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn read_os_secret(target: &str) -> Result<Option<Vec<u8>>, String> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };
    let mut target = target.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target.as_mut_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    target.zeroize();
    if ok == 0 {
        let error = unsafe { GetLastError() };
        return if error == ERROR_NOT_FOUND {
            Ok(None)
        } else {
            Err("provider credential store read failed".to_string())
        };
    }
    if credential.is_null() {
        return Err("provider credential store returned an empty record".to_string());
    }
    let bytes = unsafe {
        let record = &*credential;
        std::slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize)
            .to_vec()
    };
    unsafe { CredFree(credential.cast()) };
    Ok(Some(bytes))
}

#[cfg(windows)]
fn delete_os_secret(target: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let mut target = target.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let ok = unsafe { CredDeleteW(target.as_mut_ptr(), CRED_TYPE_GENERIC, 0) };
    target.zeroize();
    if ok == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_NOT_FOUND {
            return Err("provider credential store delete failed".to_string());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_os_secret(_target: &str, _secret: &[u8]) -> Result<(), String> {
    Err("provider credential store is unavailable".to_string())
}

#[cfg(not(windows))]
fn read_os_secret(_target: &str) -> Result<Option<Vec<u8>>, String> {
    Err("provider credential store is unavailable".to_string())
}

#[cfg(not(windows))]
fn delete_os_secret(_target: &str) -> Result<(), String> {
    Err("provider credential store is unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dirs(tag: &str) -> (PathBuf, DataDirs, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "eud-provider-secret-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        (base, dirs, home)
    }

    #[test]
    fn import_copies_only_fixed_codex_credential_and_preserves_source() {
        let (base, dirs, home) = temp_dirs("codex-import");
        let source = home.join(".codex").join("auth.json");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let bytes = br#"{"tokens":{"access_token":"secret"},"auth_mode":"chatgpt"}"#;
        fs::write(&source, bytes).unwrap();
        fs::write(home.join(".codex").join("config.toml"), b"must-not-copy").unwrap();
        let store = ProviderSecretStore::with_home(dirs.clone(), home);
        store
            .import_cli_credential(ProviderId::Codex, || Ok(()))
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert_eq!(
            fs::read(dirs.codex_home_dir().join("auth.json")).unwrap(),
            bytes
        );
        assert!(!dirs.codex_home_dir().join("config.toml").exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn failed_import_verification_rolls_back_app_copy() {
        let (base, dirs, home) = temp_dirs("rollback");
        let source = home.join(".claude").join(".credentials.json");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, br#"{"claudeAiOauth":{"accessToken":"secret"}}"#).unwrap();
        fs::create_dir_all(dirs.claude_config_dir()).unwrap();
        let destination = dirs.claude_config_dir().join(".credentials.json");
        fs::write(&destination, br#"{"claudeAiOauth":{"accessToken":"old"}}"#).unwrap();
        let store = ProviderSecretStore::with_home(dirs, home);
        assert!(store
            .import_cli_credential(ProviderId::ClaudeCode, || Err("bad token".to_string()))
            .is_err());
        assert!(String::from_utf8(fs::read(destination).unwrap())
            .unwrap()
            .contains("old"));
        fs::remove_dir_all(base).ok();
    }
}
