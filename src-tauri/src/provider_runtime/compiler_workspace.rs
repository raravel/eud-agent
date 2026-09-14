use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::config::DataDirs;

use super::ProviderRuntimeError;

/// An empty, request-owned cwd: compiler inputs travel only in the prompt.
pub struct CompilerInputWorkspace {
    root: PathBuf,
    cleaned: bool,
}

impl CompilerInputWorkspace {
    pub fn prepare(dirs: &DataDirs) -> Result<Self, ProviderRuntimeError> {
        let parent = dirs.app_local_data().join("compiler_inputs");
        fs::create_dir_all(&parent).map_err(workspace_error)?;
        let metadata = fs::symlink_metadata(&parent).map_err(workspace_error)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || crate::memory::is_reparse_point(&metadata)
        {
            return Err(ProviderRuntimeError::Transport(
                "compiler input workspace parent is not a plain directory".into(),
            ));
        }
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).map_err(workspace_error)?;
        Ok(Self {
            root,
            cleaned: false,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Call after the structured executor has released its native process owners.
    pub fn close(mut self) -> Result<(), ProviderRuntimeError> {
        self.cleaned = true;
        fs::remove_dir_all(&self.root).map_err(workspace_error)
    }
}

impl Drop for CompilerInputWorkspace {
    fn drop(&mut self) {
        if !self.cleaned {
            if let Err(error) = fs::remove_dir_all(&self.root) {
                eprintln!("eud-agent: compiler input workspace cleanup failed: {error}");
            }
        }
    }
}

fn workspace_error(error: std::io::Error) -> ProviderRuntimeError {
    ProviderRuntimeError::Transport(format!("compiler input workspace failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_cwd_is_empty_unique_and_removed_without_touching_siblings() {
        // Given: app data with unrelated persisted source and profile bytes.
        let base = std::env::temp_dir().join(format!("compiler-cwd-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        fs::create_dir_all(dirs.app_local_data()).unwrap();
        let source = dirs.app_local_data().join("source.eps");
        fs::write(&source, b"unchanged").unwrap();

        // When: concurrent compiler inputs are prepared and one owner closes.
        let first = CompilerInputWorkspace::prepare(&dirs).unwrap();
        let second = CompilerInputWorkspace::prepare(&dirs).unwrap();
        let first_root = first.root().to_path_buf();
        let second_root = second.root().to_path_buf();
        assert_ne!(first_root, second_root);
        assert_eq!(fs::read_dir(&first_root).unwrap().count(), 0);
        first.close().unwrap();

        // Then: only that owned directory is removed; dropping the other also cleans it.
        assert!(!first_root.exists());
        assert!(second_root.is_dir());
        assert_eq!(fs::read(&source).unwrap(), b"unchanged");
        drop(second);
        assert!(!second_root.exists());
        fs::remove_dir_all(base).unwrap();
    }
}
