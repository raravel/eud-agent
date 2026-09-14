//! Optional harness import omissions are review outcomes, not project failures.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A currently unavailable file or an entire store whose ownership cannot be resolved.
/// Consent binds to the scope, path and reason, so a changed failure needs renewed review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessImportIssue {
    pub id: String,
    pub scope: String,
    pub path: String,
    pub reason: String,
}

impl HarnessImportIssue {
    pub fn new(scope: &str, path: impl Into<String>, reason: impl Into<String>) -> Self {
        let path = path.into();
        let reason = reason.into();
        let mut digest = Sha256::new();
        for value in [scope, path.as_str(), reason.as_str()] {
            digest.update((value.len() as u64).to_le_bytes());
            digest.update(value.as_bytes());
        }
        Self {
            id: format!("{:x}", digest.finalize()),
            scope: scope.to_string(),
            path,
            reason,
        }
    }
}

/// Publish a complete file without replacing a destination that appeared during
/// import. Staging stays on the destination volume; callers clean it up if it
/// remains. Windows rename works on exFAT as well as NTFS, unlike hard links.
pub(crate) fn publish_staged_file(
    staged: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

        let source: Vec<u16> = staged.as_os_str().encode_wide().chain([0]).collect();
        let target: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
        if source[..source.len() - 1].contains(&0) || target[..target.len() - 1].contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "import path contains a null character",
            ));
        }
        // No REPLACE_EXISTING or COPY_ALLOWED: publication cannot overwrite a
        // concurrent file or turn into a partial cross-volume copy.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::hard_link(staged, destination)
    }
}

/// Create only plain project-local directories; never traverse a user-supplied
/// link or Windows junction when publishing trusted migration metadata.
pub(crate) fn local_state_directory(root: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let mut path = root.to_path_buf();
    for component in [".eud-agent", "state"] {
        path.push(component);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || crate::memory::is_reparse_point(&metadata)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "하네스 경로가 안전한 일반 폴더가 아닙니다: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::publish_staged_file;
    use std::fs;

    #[test]
    fn publishing_never_replaces_existing_data() {
        let root = std::env::temp_dir().join(format!("harness-publish-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let staged = root.join("staged");
        let occupied = root.join("occupied");
        fs::write(&staged, b"imported").unwrap();
        fs::write(&occupied, b"local").unwrap();
        assert!(publish_staged_file(&staged, &occupied).is_err());
        assert_eq!(fs::read(&occupied).unwrap(), b"local");
        assert_eq!(fs::read(&staged).unwrap(), b"imported");

        let available = root.join("available");
        publish_staged_file(&staged, &available).unwrap();
        assert_eq!(fs::read(&available).unwrap(), b"imported");
        fs::remove_dir_all(root).unwrap();
    }
}
