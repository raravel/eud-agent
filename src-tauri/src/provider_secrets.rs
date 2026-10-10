//! Provider credential storage. CLI credentials come only from the app's own login
//! flow: copying the user's CLI login would share its rotating OAuth refresh token.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use zeroize::Zeroize;

use crate::config::DataDirs;
use crate::provider::ProviderId;

const SECRET_TARGET_PREFIX: &str = "eud-agent/providers";

#[derive(Debug, Clone)]
pub struct ProviderSecretStore {
    dirs: DataDirs,
}

impl ProviderSecretStore {
    pub fn new(dirs: DataDirs) -> Result<Self, String> {
        Ok(Self { dirs })
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

    pub fn cli_credential_destination(&self, provider: ProviderId) -> Result<PathBuf, String> {
        match provider {
            ProviderId::Codex => Ok(self.dirs.codex_home_dir().join("auth.json")),
            ProviderId::ClaudeCode => Ok(self.dirs.claude_config_dir().join(".credentials.json")),
            _ => Err("unsupported_operation".to_string()),
        }
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

/// Owner-only POSIX mode: 0700 for directories, 0600 for files.
#[cfg(unix)]
fn harden_private_path_impl(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "provider credential path cannot be inspected".to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("provider credential path is a symbolic link".to_string());
    }
    let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| "provider credential permissions cannot be restricted".to_string())
}

#[cfg(not(any(windows, unix)))]
fn harden_private_path_impl(_path: &Path) -> Result<(), String> {
    Err("provider credential profiles require owner-only permissions".to_string())
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

/// macOS keeps provider API keys in the login Keychain as generic passwords
/// (service = credential target, account = `eud-agent`).
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "eud-agent";

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
fn write_os_secret(target: &str, secret: &[u8]) -> Result<(), String> {
    security_framework::passwords::set_generic_password(target, KEYCHAIN_ACCOUNT, secret)
        .map_err(|_| "provider credential store write failed".to_string())
}

#[cfg(target_os = "macos")]
fn read_os_secret(target: &str) -> Result<Option<Vec<u8>>, String> {
    match security_framework::passwords::get_generic_password(target, KEYCHAIN_ACCOUNT) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(_) => Err("provider credential store read failed".to_string()),
    }
}

#[cfg(target_os = "macos")]
fn delete_os_secret(target: &str) -> Result<(), String> {
    match security_framework::passwords::delete_generic_password(target, KEYCHAIN_ACCOUNT) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(_) => Err("provider credential store delete failed".to_string()),
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn write_os_secret(_target: &str, _secret: &[u8]) -> Result<(), String> {
    Err("provider credential store is unavailable".to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn read_os_secret(_target: &str) -> Result<Option<Vec<u8>>, String> {
    Err("provider credential store is unavailable".to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn delete_os_secret(_target: &str) -> Result<(), String> {
    Err("provider credential store is unavailable".to_string())
}
