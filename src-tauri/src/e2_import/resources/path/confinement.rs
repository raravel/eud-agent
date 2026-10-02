use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::super::super::{
    parser::MAX_PATH_BYTES, E2Feature, E2Issue, E2IssueCode, E2IssueSeverity, ProjectResourceKind,
};
use super::super::E2ResourceResolveError;
use super::SelectedFile;
pub(crate) fn allowed_extensions(kind: ProjectResourceKind) -> &'static [&'static str] {
    match kind {
        ProjectResourceKind::Map => &["scx", "scm"],
        ProjectResourceKind::Audio => &["ogg", "wav"],
        ProjectResourceKind::Bin => &["bin"],
        ProjectResourceKind::Tile => &["bmp", "bin"],
        ProjectResourceKind::Plugin => &["eps", "py"],
        ProjectResourceKind::Dat => &["dat", "json"],
        ProjectResourceKind::File => &["tbl", "bin", "dat", "iscript", "aiscript"],
    }
}

pub(crate) fn require_extension(
    kind: ProjectResourceKind,
    candidate: &SelectedFile,
    path: &Path,
) -> Result<(), E2ResourceResolveError> {
    if allowed_extensions(kind)
        .iter()
        .any(|allowed| candidate.extension.eq_ignore_ascii_case(allowed))
    {
        Ok(())
    } else {
        Err(invalid(
            "file extension does not match the requested resource kind",
            Some(path),
        ))
    }
}

pub(crate) fn validate_existing_file(
    path: &Path,
    feature: Option<E2Feature>,
) -> Result<PathBuf, E2ResourceResolveError> {
    if path.as_os_str().to_string_lossy().len() > MAX_PATH_BYTES {
        return Err(invalid("path exceeds 4,096 bytes", Some(path)));
    }
    ensure_no_links(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_issue("required resource does not exist", path, error))?;
    if crate::memory::is_untrusted_link(&metadata) || !metadata.is_file() {
        let mut issue = issue("resource must be a regular non-link file", Some(path));
        issue.feature = feature;
        return Err(E2ResourceResolveError::Fatal(issue));
    }
    fs::canonicalize(path).map_err(|error| io_issue("cannot canonicalize resource", path, error))
}

pub(crate) fn read_bounded(
    path: &Path,
    limit: u64,
    is_cancelled: &impl Fn() -> bool,
) -> Result<Vec<u8>, E2ResourceResolveError> {
    let mut file =
        fs::File::open(path).map_err(|error| io_issue("cannot open resource", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_issue("cannot inspect open resource", path, error))?;
    if metadata.len() > limit {
        return Err(invalid("resource exceeds its byte limit", Some(path)));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| invalid("resource size cannot be represented", Some(path)))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        check_cancelled(is_cancelled)?;
        let count = file
            .read(&mut chunk)
            .map_err(|error| io_issue("cannot read resource", path, error))?;
        if count == 0 {
            break;
        }
        let next = u64::try_from(bytes.len().saturating_add(count))
            .map_err(|_| invalid("resource size cannot be represented", Some(path)))?;
        if next > limit {
            return Err(invalid(
                "resource changed beyond its byte limit",
                Some(path),
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

pub(crate) fn stored_path(
    package_root: &Path,
    value: &str,
) -> Result<PathBuf, E2ResourceResolveError> {
    validate_path_text(value)?;
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("relative resource path contains traversal", None));
    }
    Ok(package_root.join(path))
}

pub(crate) fn path_identity(path: &Path) -> Result<String, E2ResourceResolveError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| io_issue("cannot resolve current directory", path, error))?
            .join(path)
    };
    let identity = if absolute.exists() {
        fs::canonicalize(&absolute)
            .map_err(|error| io_issue("cannot canonicalize path identity", &absolute, error))?
    } else {
        absolute
    };
    Ok(super::inventory::case_fold(
        &identity.to_string_lossy().replace('\\', "/"),
    ))
}

pub(crate) fn check_cancelled(
    is_cancelled: &impl Fn() -> bool,
) -> Result<(), E2ResourceResolveError> {
    if is_cancelled() {
        Err(E2ResourceResolveError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn invalid(message: &str, path: Option<&Path>) -> E2ResourceResolveError {
    E2ResourceResolveError::Fatal(issue(message, path))
}

pub(crate) fn ensure_no_links(path: &Path) -> Result<(), E2ResourceResolveError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| io_issue("cannot resolve current directory", path, error))?
            .join(path)
    };
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if crate::memory::is_untrusted_link(&metadata) => {
                return Err(invalid(
                    "path contains a symlink or reparse point",
                    Some(ancestor),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_issue("cannot inspect path component", ancestor, error)),
        }
    }
    Ok(())
}

pub(crate) fn validate_path_text(value: &str) -> Result<(), E2ResourceResolveError> {
    if value.is_empty() || value.contains(['\0', '\r', '\n']) || value.len() > MAX_PATH_BYTES {
        return Err(invalid("resource path is empty or malformed", None));
    }
    Ok(())
}

pub(crate) fn read_directory(path: &Path) -> Result<Vec<PathBuf>, E2ResourceResolveError> {
    fs::read_dir(path)
        .map_err(|error| io_issue("cannot read package directory", path, error))?
        .map(|entry| {
            entry
                .map(|value| value.path())
                .map_err(|error| io_issue("cannot read package entry", path, error))
        })
        .collect()
}

fn issue(message: &str, path: Option<&Path>) -> E2Issue {
    E2Issue {
        code: E2IssueCode::InvalidResource,
        severity: E2IssueSeverity::Fatal,
        message: message.to_string(),
        path: path.map(|value| value.to_string_lossy().into_owned()),
        feature: None,
        need_id: None,
    }
}

pub(crate) fn io_issue(
    context: &str,
    path: &Path,
    error: std::io::Error,
) -> E2ResourceResolveError {
    invalid(&format!("{context}: {error}"), Some(path))
}
