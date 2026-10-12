use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::super::super::E2SourceKind;
use super::super::E2ResourceResolveError;
use super::confinement::{
    check_cancelled, ensure_no_links, invalid, io_issue, path_identity, read_directory,
};

pub(crate) fn fingerprint_package_inventory(
    package_root: &Path,
    source_kind: E2SourceKind,
    hasher: &mut Sha256,
    is_cancelled: &impl Fn() -> bool,
) -> Result<(), E2ResourceResolveError> {
    let mut roots = vec![package_root.to_path_buf()];
    if source_kind == E2SourceKind::E2p {
        for folder in ["Map", "Resource", "Sound", "sound"] {
            roots.push(package_root.join(folder));
        }
    }
    roots.sort_by_key(|path| case_fold(&path.to_string_lossy()));
    roots.dedup_by(|left, right| path_identity(left).ok() == path_identity(right).ok());
    for root in roots {
        check_cancelled(is_cancelled)?;
        let metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_issue("cannot inspect package directory", &root, error)),
        };
        if crate::memory::is_untrusted_link(&metadata) || !metadata.is_dir() {
            if root == package_root {
                return Err(invalid(
                    "E2 package root is not a safe directory",
                    Some(&root),
                ));
            }
            continue;
        }
        ensure_no_links(&root)?;
        let mut entries = read_directory(&root)?;
        entries.sort_by_key(|path| case_fold(&path.to_string_lossy()));
        let mut folded = BTreeSet::new();
        for path in entries {
            check_cancelled(is_cancelled)?;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| invalid("package entry name must be UTF-8", Some(&path)))?;
            if !folded.insert(case_fold(name)) {
                return Err(invalid(
                    "package entries collide when case is ignored",
                    Some(&path),
                ));
            }
            let entry_metadata = fs::symlink_metadata(&path)
                .map_err(|error| io_issue("cannot inspect package entry", &path, error))?;
            if crate::memory::is_untrusted_link(&entry_metadata) {
                return Err(invalid(
                    "package inventory contains a symlink or reparse point",
                    Some(&path),
                ));
            }
            hash_part(hasher, name.as_bytes());
            hash_part(hasher, &entry_metadata.len().to_le_bytes());
            hash_part(hasher, &[u8::from(entry_metadata.is_file())]);
        }
    }
    Ok(())
}

pub(crate) fn hash_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(bytes.len().to_le_bytes());
    hasher.update(bytes);
}

pub(crate) fn case_fold(value: &str) -> String {
    value.to_lowercase()
}
